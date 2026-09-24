//! The owned skin palette: benilla computes every skinned rig's joint palette
//! itself and uploads it to a region of the shared light buffer; `wow_model.wgsl`'s vertex stage
//! skins from it. This replaces Bevy's `SkinnedMesh` lane end to end — `extract_skins`'
//! world-wide `Changed<GlobalTransform>` sweep and `prepare_skins`' full-staging-buffer rewrite
//! (both O(everything), measured in decision 0718) become no-ops because no entity carries
//! `SkinnedMesh` anymore (and no mesh carries Bevy's joint attributes, so the `SKINNED`
//! pipeline path never compiles).
//!
//! The shape mirrors the interior-prop probe table (`lighting::prop_probes`), the house pattern:
//! a main-world slab ([`RigPalettes`]) whose slots free themselves through a component hook
//! ([`RigSkin`]'s `on_replace`), `Arc`-shared rows so the render-world extract is a pointer bump,
//! and a `PrepareResources` upload that writes only what changed — here per-rig **dirty ranges**
//! (a parked, stationary rig re-uploads nothing) plus the slot table on allocation changes.
//!
//! **Palettes are RIG-RELATIVE**: row `b` of rig `r` is
//! `rig_from_joint(b) × inverse_bindpose(b)`, where `rig_from_joint` is the joint's world frame
//! with the rig's own world origin subtracted off — same 3×3 as Bevy's `skin_model` would
//! produce, translation measured from the rig root instead of the map's. The origin itself rides
//! a parallel per-slot table ([`rig_origin_region_offset`]), and the vertex stage
//! adds it back **camera-relative**, so a vertex never exists as an absolute world coordinate in
//! f32. That is the whole point: at Elwynn's ~9.5 k-yard coordinates one f32 ULP is ~1 mm, the
//! bone chain and the palette each spend one, and because the pose is recomposed every frame the
//! rounding is *fresh noise every frame* — the long-standing character shimmer. Rig-relative
//! numbers are ~1 yard, where an ULP is ~0.1 µm. (Rows stay 3 `vec4`s — the affine's rows; the
//! fourth is implicit `[0,0,0,1]`.)
//!
//! The vertex stage reads the part's rig slot from its `MeshTag` rig field (`mesh_tag::rig_bits`),
//! resolves the rig's base bone index through the slot table, and blends the four indexed bones'
//! rows by the vertex weights — replacing (not composing with) the mesh's world matrix, exactly
//! like Bevy skinning, then offsets by `origin − camera`.
//!
//! Both writers keep the convention — the collapsed lane ([`RigPalettes::write_rig_worlds`]) is
//! handed frames already composed from a zero-translation root, so its rows are exact; the entity
//! lane ([`RigPalettes::write_rig`]) subtracts the holder's world translation off globals Bevy
//! propagated in absolute space, so its rows inherit whatever rounding propagation already spent
//! (a named residual — killing it needs a floating origin, not a palette change).
//!
//! **Dirtiness is structural, not assumed**: every joint entity carries a [`RigJoint`] marker
//! (planted by [`spawn_joints`]), and [`compute_rig_palettes`] runs one linear
//! `Changed<GlobalTransform>` pass over exactly those entities — so *every* writer of a joint's
//! world pose (the 0712 evaluator, the body twist, global-sequence channels, the billboard joint
//! pass, a mount's seat, the terrain-conform node, plain movement) marks its rig by construction.
//! The pass is rig-scoped (≈ the bone population), unlike the Bevy sweep it replaces (the whole
//! world); Stage B of 0720 replaces it with writer-driven dirtying when the bone entities
//! themselves collapse.

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

/// Bone capacity of the palette region — concurrently-resident skinned bones across every live
/// rig (units, animated doodads, spell effects, booths). The LBRS census holds ~59 k unit
/// bones + ~1.6 k doodad-rig bones; 128 k is head-room, at 48 B/bone = 6 MB of the shared buffer.
/// Mirrored by `wow_model.wgsl`'s region declaration — keep in sync.
pub(crate) const MAX_PALETTE_BONES: usize = 131_072;

/// Bytes per bone in the palette region: 3 × vec4 rows of the affine.
const BONE_BYTES: u64 = 48;

/// Byte offset of the rig slot table inside the shared light buffer (after the prop-probe
/// region). One u32 per slot: the rig's base bone index.
pub(crate) fn rig_table_region_offset() -> u64 {
    crate::lighting::prop_probe_region_offset() + (7 * crate::lighting::MAX_PROP_PROBES * 16) as u64
}

/// Byte offset of the per-slot rig ORIGIN table — one `vec4` per slot, `xyz` =
/// the world position the rig's rows are measured from (`w` unused). Sits after the instance-tint
/// table, mirroring `wow_model.wgsl`'s struct order.
pub(crate) fn rig_origin_region_offset() -> u64 {
    crate::instance_tint::region_offset() + crate::instance_tint::region_bytes()
}

/// Bytes the origin table adds (32 KB at 2048 slots).
pub(crate) fn rig_origin_region_bytes() -> u64 {
    MAX_RIG_SLOTS as u64 * 16
}

/// Byte offset of the palette rows themselves — after the slot table, the per-instance tint table
/// that shares this slot index ([`crate::instance_tint`]), the origin table, and
/// the mat-anim delta table ([`crate::mat_anim_table`]), and the straddle clip
/// table ([`crate::straddle`]). The rows stay last because `wow_model.wgsl`
/// declares them as the struct's one runtime-sized array.
pub(crate) fn palette_region_offset() -> u64 {
    crate::straddle::region_offset() + crate::straddle::region_bytes()
}

/// Total bytes the slot-indexed regions add to every `wow_light`-layout buffer
/// (`lighting::light_blob_bytes`): the rig slot table, the instance-tint table, the origin table,
/// the mat-anim table, the straddle clip table, the palette rows.
pub(crate) fn palette_regions_bytes() -> u64 {
    (MAX_RIG_SLOTS * 4) as u64
        + crate::instance_tint::region_bytes()
        + rig_origin_region_bytes()
        + crate::mat_anim_table::region_bytes()
        + crate::straddle::region_bytes()
        + MAX_PALETTE_BONES as u64 * BONE_BYTES
}

/// The main-world palette table: rows + slot table behind `Arc` (the extract is a pointer bump),
/// a slot slab and a coalescing bone-range free list, and the frame's dirty ranges.
#[derive(Resource)]
pub struct RigPalettes {
    /// `3 × MAX_PALETTE_BONES` vec4 rows — the CPU mirror (the mouseover picker reads it).
    rows: Arc<Vec<[f32; 4]>>,
    /// The row buffer retired by the last clone (`rows_make_mut`), reused once unshared.
    spare: Option<Arc<Vec<[f32; 4]>>>,
    /// Slot → base bone index. Slot 0 is the tag's "no rig" sentinel and never allocated.
    table: Arc<Vec<u32>>,
    /// Slot → the world position this rig's rows are measured from; `w` unused.
    /// Written by the same call that writes the rows, so the pair can never disagree.
    origins: Arc<Vec<[f32; 4]>>,
    /// Bumped whenever any origin changes — the origin table's upload gate for the shared buffer.
    origin_generation: u64,
    /// Bumped only when a MIRRORED slot's origin changes, so a world full of walking creatures
    /// does not re-push 32 KB into every booth buffer each frame.
    origin_mirror_generation: u64,
    /// Per-slot bone count; 0 = free slot.
    slot_len: Vec<u32>,
    free_slots: Vec<u16>,
    slot_high: usize,
    /// Free bone ranges `(base, len)`, kept sorted by base and coalesced on free.
    free_ranges: Vec<(u32, u32)>,
    /// Per slot: whether this rig's rows must ALSO reach the booth mirror buffers
    /// ([`RigPaletteMirrors`]) — set for booth rigs only, so the world's rig traffic never
    /// multiplies across the studio buffers.
    mirrored: Vec<bool>,
    /// Bone ranges rewritten since the last publish; `.2` = the owning slot was mirrored.
    dirty: Vec<(u32, u32, bool)>,
    /// Bumped on every alloc/free — the slot table's upload gate.
    table_generation: u64,
    /// Peak concurrent slots / bones (the overflow diagnostic — near capacity ⇒ grow).
    peak_slots: usize,
    peak_bones: u32,
    live_bones: u32,
    /// Allocations refused since the last census line (`WOW_RIG_CENSUS`) — the exhaustion
    /// diagnosis's live denial rate. Read-and-reset by [`census_rig_palettes`].
    denied: u32,
    /// Has the exhaustion warn already been logged this session? A latch, not a counter, and
    /// deliberately never reset: [`Self::denied`] is `mem::take`n by the census, so gating the
    /// warn on it would re-fire once per census window instead of once per session (2264).
    denied_warned: bool,
    /// `WOW_RIG_COST` meters (decision 0736 premise check): whole-vec deep copies this frame
    /// (`Arc::make_mut` clones when the extract still holds last publish's reference), the µs
    /// they took, and the rows (bones) written. Printed + reset by [`publish_rig_palettes`].
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

/// `WOW_RIG_COST=1`: the rig lane's per-frame cost split — copy vs compose vs upload — in
/// grep-and-average lines beside `FPS_PROBE` (the 0736 premise meter; the 0734 lesson made
/// premise checks law). Zero-cost when off: one env read, once.
pub fn rig_cost_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_RIG_COST").is_some())
}

/// Copy-on-write on the shared rows, timing the calls that actually copy: between a publish and
/// the frame's first write the extract holds a second reference, so that write must clone. A
/// static helper (not a method) so call sites keep their split-field borrows.
///
/// **The clone is bounded by the bone watermark, never the capacity**.
/// `Arc::make_mut` cloned the whole 3×131072-row vec — ~6.3 MB, once per frame, at every spot in
/// the world, however few rigs were awake (1473's fixed-tax finding). A fresh zeroed vec is a
/// calloc — the kernel hands back zero pages untouched — so the real cost is the live-prefix
/// memcpy alone: ~250 KB at a busy pin. Everything at or above the watermark is bitwise zero by
/// invariant (see [`RigPalettes::bone_watermark`]), so the prefix IS the whole content.
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
        // The retired buffer from two publishes ago, once the render world has let go of it:
        // copy the live prefix in and take it back. Its tail is stale — and never read: the GPU
        // only ever receives dirty ranges, so what the CPU vector holds past the watermark
        // reaches nothing (the calloc'd zeros it replaced were never uploaded either). A fresh
        // allocation only while the render world still holds both.
        let mut new = match spare.take().and_then(|a| Arc::try_unwrap(a).ok()) {
            Some(v) if v.len() == rows.len() => v,
            _ => vec![[0.0f32; 4]; rows.len()],
        };
        new[..live].copy_from_slice(&rows[..live]);
        *spare = Some(std::mem::replace(rows, Arc::new(new)));
        *copy_us += t.elapsed().as_secs_f32() * 1e6;
        *copies += 1;
    }
    // Uniquely owned now — `make_mut` is a plain borrow.
    Arc::make_mut(rows)
}

fn rebase(mut world: Affine3A, origin: Vec3) -> Affine3A {
    world.translation -= bevy::math::Vec3A::from(origin);
    world
}

/// [`rebase`]'s `GlobalTransform` face — for the lanes that hand
/// [`RigPalettes::write_rig_worlds`] frames Bevy propagated in absolute space, and for the
/// collapsed lane's root (which composes its chain *from* the rebased root).
pub(crate) fn rebase_global(g: GlobalTransform, origin: Vec3) -> GlobalTransform {
    GlobalTransform::from(rebase(g.affine(), origin))
}

/// The origin a rig's rows are measured from, given its root's world translation — the ONE place
/// the decision-0974 rebase can be switched off, so no caller can hold half the convention.
///
/// `WOW_NO_RIG_REBASE=1` is the A/B lever (the `WOW_NO_MOUNT_TILT` shape): it makes every rig's
/// origin the *map* origin, so the chain composes in absolute world space and the rows carry
/// ~9 k-yard translations again — the pre-0974 f32 rounding, and its shimmer, back on animated
/// characters. The shader needs no branch for it: `frame · v + (0 − camera)` spends the ULP inside
/// the big-times-small product exactly where the old route spent it inside `clip_from_world ×
/// p_world`. Zero-cost when unset: one env read, once.
pub(crate) fn rebase_origin(root_world: Vec3) -> Vec3 {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    match *ON.get_or_init(|| std::env::var_os("WOW_NO_RIG_REBASE").is_none()) {
        true => root_world,
        false => Vec3::ZERO,
    }
}

impl RigPalettes {
    /// Claim a slot + a bone range for a rig. `None` when either slab is exhausted — the caller
    /// The bone offset above which every row is bitwise ZERO — the COW clone's bound (1479).
    ///
    /// The invariant, and why it holds: [`Self::alloc`] is first-fit over a sorted, coalescing
    /// free list, so allocations pack low; [`Self::free`] zeroes the rows it returns **before**
    /// the range re-enters the free list (its own `rows_make_mut` call still counts the range as
    /// live, so the zeroing lands inside the copied prefix); and rows above everything ever
    /// allocated are zeros from construction. So a tail free range that touches capacity is all
    /// zeros, and its base bounds the content.
    fn bone_watermark(&self) -> u32 {
        match self.free_ranges.last() {
            Some(&(base, len)) if base + len == MAX_PALETTE_BONES as u32 => base,
            _ => MAX_PALETTE_BONES as u32,
        }
    }

    /// warns and falls back to the static (bind-pose) mesh, graceful and visible in the log
    /// rather than wrong.
    pub(crate) fn alloc(&mut self, bones: u32) -> Option<(u16, u32)> {
        if bones == 0 {
            return None;
        }
        // First-fit range.
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
        // A fresh range starts at zero rows: the spare-buffer reuse (`rows_make_mut`) means the
        // CPU mirror above the watermark is a previous occupant's pose, and three readers
        // (`write_rig`'s torn-joint keep, `write_rider`'s unchanged compare, the computed-rig
        // readbacks) assume an unallocated row is zero (review 2026-09-04).
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

    /// Return a rig's slot + range. Rows are zeroed (a part's stale tag during the one-frame
    /// despawn skew reads a degenerate matrix — collapsed at the origin — never another rig's
    /// pose), and the range coalesces back into the free list.
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
        // Watermark BEFORE the range re-enters the free list — the zeroing below must land
        // inside the copied prefix (the invariant's own ordering, [`Self::bone_watermark`]).
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
        // …and the origin with them, so "collapsed at the origin" stays literally true for a
        // stale tag during the one-frame despawn skew.
        self.set_origin(slot, Vec3::ZERO);
        // Insert sorted + coalesce with both neighbours.
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

    /// Publish the world position slot `slot`'s rows are measured from. Called by
    /// both row writers with the same origin they subtracted, so the pair is atomic by
    /// construction — there is no frame where the shader can add a stale origin to fresh rows.
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

    /// Rewrite one rig's rows from its joints' world transforms × inverse bindposes, and mark the
    /// range dirty. `n` is clamped to the allocated length (a malformed model whose joint list
    /// and bindpose list disagree writes the shorter).
    ///
    /// `origin` is the rig holder's world translation: rows are written RELATIVE to it (decision
    /// 0974) and the vertex stage adds it back. Unlike the collapsed lane, these globals were
    /// propagated by Bevy in absolute world space, so subtracting the origin here cannot recover
    /// precision propagation already spent — it buys the *clip-space* half of the fix only. Both
    /// lanes must still agree on the convention, which is why it is not optional.
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

    /// The collapsed-rig lane's row write: the world pass hands the composed —
    /// and billboard-replaced — per-bone frames directly; rows are `frame × ibp`, the same product
    /// [`Self::write_rig`] reads off joint entities. `worlds` beyond the allocation (or the
    /// bindpose list) is ignored, mirroring the entity path's clamp.
    ///
    /// The frames arrive already **rig-relative** — the world pass composes the
    /// chain from a zero-translation root — so nothing here spends a ~9 k-yard ULP and the rows
    /// are exact to a rig-sized quantity. `origin` is the world position they are measured from;
    /// the vertex stage adds it back camera-relative.
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

    /// **The rider write**: fill every row of `slot` with ONE rigid frame, and
    /// publish the world position it is measured from.
    ///
    /// An attached model rests at bind pose, where a palette row `world_from_joint × ibp` is the
    /// placement matrix for every joint (`F × bind_j × bind_j⁻¹`) — so the whole model is one
    /// frame repeated, and no inverse-bindpose list is consulted or needed. `frame` arrives in the
    /// HOST rig's frame, at rig-sized magnitudes; `origin` is the host's world position, which the
    /// vertex stage adds back camera-relative. The point of the whole lane is that the two are
    /// never summed in f32 on the CPU (see [`crate::rig_rider`]).
    ///
    /// **Idempotent, deliberately.** Riders are re-derived every frame from the host's pose, so a
    /// standing unit would otherwise dirty its rows — and pay an upload — for a value that has not
    /// moved. The compare is bit equality, not an epsilon, exactly as the camera's own no-op write
    /// gate is: a real sub-epsilon change must still land.
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

    /// Flag a rig's rows to ALSO reach the booth mirror buffers ([`RigPaletteMirrors`]): the
    /// booth spawners call it for their rigs — whose materials bind a studio light buffer, not
    /// the shared one — right after allocation.
    pub fn mark_mirrored(&mut self, slot: u16) {
        if let Some(m) = self.mirrored.get_mut(slot as usize) {
            *m = true;
        }
        // Push the ORIGIN table to the mirrors on this edge, unconditionally. The
        // mirror upload is gated on `origin_mirror_generation`, which only moves when a slot that
        // is ALREADY mirrored changes — so a booth rig whose origin was written before this call
        // (or written again to the same value after it) would leave the studio buffers holding a
        // zero origin, and the booth's subject would render an Elwynn's-worth of yards away.
        self.origin_mirror_generation += 1;
    }

    /// The mouseover picker's read side: rig slot → **world-space** `Mat4` palette (the same
    /// values Bevy's `skin_model` would compute), reconstructed from the rows. The stored rows are
    /// rig-relative, so the slot's origin goes back on here — the picker ray is a
    /// world-space ray, and it is not a precision consumer.
    pub fn world_palette(&self, slot: u16, bones: usize) -> Option<Vec<Mat4>> {
        let o = *self.origins.get(slot as usize)?;
        self.rows_at(slot, bones, [o[0], o[1], o[2]])
    }

    /// The rig's palette rows **as the vertex stage blends them** — rig-relative,
    /// the slot origin deliberately NOT added back. [`Self::world_palette`] is the picker's
    /// world-space read; this is the PRECISION one, for the same reason [`Self::rider_placement`]
    /// returns its pair unsummed: re-adding the ~9 k-yard origin would stamp the very f32 grid
    /// the lane exists to avoid onto the measurement, and the instrument would then read its own
    /// arithmetic as the defect.
    pub fn rig_rows(&self, slot: u16, bones: usize) -> Option<Vec<Mat4>> {
        self.rows_at(slot, bones, [0.0; 3])
    }

    /// Both reads above: the rows of `slot`, with `o` added onto the translation column.
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

    /// A slot's rig ORIGIN — the world position its rows are measured from, exactly as the vertex
    /// stage reads it out of `rig_origin[slot]`. The instrument pairs it with [`Self::rig_rows`] to
    /// reproduce the shader's own `frame_from_local * v + (frame_origin - view.world_position)`
    /// without ever forming the absolute world coordinate the lane exists to avoid.
    pub fn slot_origin(&self, slot: u16) -> Option<Vec3> {
        let o = self.origins.get(slot as usize)?;
        Some(Vec3::new(o[0], o[1], o[2]))
    }

    /// A rider slot's placement **as the vertex stage reads it**: its
    /// `(rig_origin, row-0 translation)` pair, unsummed. The instrument read — `WOW_JITTER`
    /// measures the row's own motion against the anchor's absolute world translation, and the
    /// whole point of the lane is that only the first of those two is on an f32 grid coarse
    /// enough to see. Summing them here would destroy exactly the thing being measured, which is
    /// why this returns the pair and [`Self::world_palette`] (a picker read, not a precision one)
    /// does not.
    pub fn rider_placement(&self, slot: u16) -> Option<(Vec3, Vec3)> {
        self.row_placement(slot, 0)
    }

    /// The same pair for an arbitrary bone of a slot. A bind-pose rider repeats one frame across
    /// every row, so row 0 answers for the model; a **posed** one (the flexing
    /// ranged prop) does not, and asking which bone is the only way to see that its rows differ at
    /// all. `None` for an unallocated slot or a bone past its length.
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

    /// How many live rigs have a COMPUTED palette (their first bone's first row is non-zero — a
    /// freshly allocated or never-computed rig reads all-zero rows). The FPS probe prints it:
    /// allocation without computation renders origin-collapsed rigs, and no other probe number
    /// would catch that failure mode.
    pub fn computed_rigs(&self) -> usize {
        (0..self.slot_high)
            .filter(|&s| self.slot_len[s] > 0 && self.rows[3 * self.table[s] as usize] != [0.0; 4])
            .count()
    }

    /// Live and peak occupancy `(slots, bones, peak_slots, peak_bones)` — the overflow diagnostic.
    pub fn occupancy(&self) -> (usize, u32, usize, u32) {
        (
            self.slot_high - 1 - self.free_slots.len(),
            self.live_bones,
            self.peak_slots,
            self.peak_bones,
        )
    }

    /// Slots still allocatable right now — the pressure signal: the doodad
    /// reaper starts reclaiming parked rigs under [`crate::doodad_anim::REAP_LOW_WATER`], and
    /// the unit heal only rebuilds a starved visual when there is real room to succeed.
    pub fn slot_headroom(&self) -> usize {
        MAX_RIG_SLOTS - self.slot_high + self.free_slots.len()
    }
}

/// The unit whose visual was built WITHOUT a palette rig because the table was full at attach
/// time — it renders the static bind pose. [`crate::entities`]' heal system
/// rebuilds it (the 0x60abe0 display-swap teardown, `Reattached` so it doesn't re-fade) once the
/// table has headroom again; without this marker the denial was PERMANENT, the "statue mobs at
/// the stream-in boundary" bug.
#[derive(Component)]
pub struct RigStarved;

/// A registered rig, on the entity that owns its pose (the unit / doodad-host / booth root):
/// its palette slot + the joint entities + the shared inverse bindposes. The hook frees the slot
/// whoever despawns the rig — and `on_replace` (not `on_remove`) because a gear/display rebuild
/// re-runs the attach on the SAME entity and insert-overwrites this component: `on_remove` never
/// fires on an overwrite, and the old slot would leak until the unit despawned.
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
    /// The rig's allocated bone count — the palette-facing length (the mouseover picker sizes
    /// its palette read with it; the collapsed lane has no joint list to measure).
    pub fn bones(&self) -> u32 {
        self.len
    }

    /// The rig's inverse bindposes — the instrument read. `WOW_JITTER`'s palette term probes the
    /// rows at each bone's BIND position, which is the only honest way to ask "how far does a
    /// vertex bound to this bone actually move" of the numbers the GPU blends: a row's own
    /// translation column is the bind-space ORIGIN's image, a lever up to the model's full height
    /// away from the bone, and differencing it reads a rotation as ~10x the displacement any real
    /// vertex sees.
    pub fn ibp(&self) -> &Handle<SkinnedMeshInverseBindposes> {
        &self.ibp
    }

    /// Allocate a palette rig for the collapsed lane: no joint entities — the
    /// world pass hands composed frames to [`RigPalettes::write_rig_worlds`] instead, so the
    /// entity-lane change sweep never touches this slot (empty `joints` make its `write_rig` a
    /// no-op by construction).
    pub fn allocate_bones(
        palettes: &mut RigPalettes,
        bones: u32,
        ibp: Handle<SkinnedMeshInverseBindposes>,
    ) -> Option<Self> {
        Self::allocate_inner(palettes, bones, Vec::new(), ibp)
    }

    /// Allocate a palette rig over live joint entities (the effect/booth/equipment lane — the
    /// change sweep computes its rows; doodads moved to [`Self::allocate_bones`]).
    /// `None` (with one loud warn per session — a promise this doc made for a long time before
    /// the code kept it, 2264) when the table is full — the caller renders the
    /// static bind-pose mesh instead.
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
                // **Once per session, as this function's doc has always promised.** It did not
                // keep that promise: the denial path warned on every call, and the lazy-doodad
                // caller retries *every frame while the host is drawn*
                // (`doodad_anim::lazy` — "table full — the warn fired; the gate retries while
                // drawn"), so a crowded scene turned one fact into a per-frame flood that buries
                // every other warning in the log. The repeats were never the signal anyway:
                // `denied` is already counted here and already reported by the palette census.
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
    // The slot indexes the per-instance TINT table too, and slots are recycled — so
    // clear it on the same edge that frees it. Structural, not disciplinary: whatever kills a rig
    // (despawn, gear/display rebuild) cannot leave a dead unit's colour behind for the next unit
    // that allocates the slot, and no tinted-unit teardown path has to remember to.
    if let Some(mut tints) = world.get_resource_mut::<crate::instance_tint::InstanceTints>() {
        tints.clear(slot);
    }
    // …and the straddle split's waterline, on the same index for the same reason:
    // a recycled slot must not clip the next unit at the last one's water.
    if let Some(mut clips) = world.get_resource_mut::<crate::straddle::WaterClips>() {
        clips.clear(slot);
    }
}

/// On every joint entity (planted by [`spawn_joints`]): the rig root whose palette this
/// joint feeds. The change sweep below is a linear pass over exactly these.
#[derive(Component)]
pub(crate) struct RigJoint(pub(crate) Entity);

/// On every skinned part entity: the rig root, for CPU-side palette consumers (the mouseover
/// picker's skinned ray test). The GPU side reads the same link from the part's `MeshTag` rig
/// field instead.
#[derive(Component)]
pub struct RigPart(pub Entity);

/// PostUpdate, after the billboard joint pass (the last joint-world writer): one linear
/// `Changed<GlobalTransform>` sweep over the joint population → the set of rigs whose palettes
/// must recompute this frame → rewrite exactly those rigs' rows.
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
        // The rig holder's own world translation is the rebasing origin: every
        // joint is a descendant of it, so the rows come out rig-sized. Any origin would be
        // *correct* — only precision cares that it sits near the geometry — so a holder without a
        // global (mid-teardown) safely falls back to the map origin, i.e. the old behaviour.
        let origin = rebase_origin(
            worlds
                .get(root)
                .map(|g| g.translation())
                .unwrap_or_default(),
        );
        palettes.write_rig(rig, ibp, origin, &worlds);
    }
}

/// The render-world mirror: `Arc` bumps + the frame's dirty ranges.
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

/// Main world, after the compute: hand the frame's dirty ranges + the shared rows to extraction.
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

/// Extra `wow_light`-layout buffers that mirror the palette regions (the glue/portrait booths'
/// studio buffers — their skinned rigs bind those, not the shared buffer). Registered by key at
/// booth creation; re-registration replaces (a rebuilt booth's old buffer drops).
#[derive(Resource, Clone, Default, ExtractResource)]
pub struct RigPaletteMirrors(
    pub std::collections::HashMap<&'static str, bevy::render::render_resource::Buffer>,
);

/// What the last upload actually put on the GPU — the four independent change gates
/// ([`upload_rig_palettes`]). `dirty` is an `Arc` pointer, not a counter: a new publish makes a
/// new `Arc`.
#[derive(Default)]
struct UploadedGenerations {
    table: Option<u64>,
    dirty: u64,
    origin: Option<u64>,
    origin_mirror: Option<u64>,
}

/// Render world (`PrepareResources`): write the dirty rows + (on change) the slot table and the
/// rig-origin table into the shared buffer and every registered mirror. A parked, stationary world
/// costs zero bytes here.
fn upload_rig_palettes(
    queue: Res<RenderQueue>,
    shared: Option<Res<crate::lighting::SharedLightBuffer>>,
    mirrors: Option<Res<RigPaletteMirrors>>,
    data: Option<Res<RigPaletteExtract>>,
    mut last: Local<UploadedGenerations>,
) {
    let Some(data) = data else { return };
    // The extract clones every frame; gate on content. `dirty` counts published dirty batches
    // via the Arc pointer (a new publish makes a new Arc).
    let dirty_ptr = Arc::as_ptr(&data.dirty) as u64;
    let table_new = last.table != Some(data.table_generation);
    let dirty_new = last.dirty != dirty_ptr && !data.dirty.is_empty();
    // The origin table is 32 KB written whole: tiny beside the rows, and the
    // mirrors take their own generation so a world full of walking creatures does not re-push it
    // into every booth buffer each frame.
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
    // Coalesce the per-rig dirty ranges before touching the queue: rigs allocate contiguously,
    // so a steady frame's ~750 one-rig ranges merge into a handful of runs — the 0724 ledger
    // measured the per-range `write_buffer` loop at 2.7 ms/frame, almost all call overhead.
    // Small gaps (a parked body between two live ones) are bridged for the same reason
    // (`COALESCE_GAP_BONES`).
    let all = coalesce_ranges(data.dirty.iter().map(|&(b, l, _)| (b, l)).collect());
    let mirrored_only = coalesce_ranges(
        data.dirty
            .iter()
            .filter(|&&(_, _, m)| m)
            .map(|&(b, l, _)| (b, l))
            .collect(),
    );
    // Every target sees the slot table; the shared buffer takes all dirty rows, the booth
    // mirrors only the ranges flagged mirrored (their rigs) — world traffic never multiplies.
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

/// The gap (in bones) two dirty runs may leave between them and still upload as ONE
/// `write_buffer`. A `queue.write_buffer` is not a memcpy: wgpu allocates a staging buffer per
/// call, registers it, records the copy and releases it — a few microseconds of fixed cost
/// against ~10 GB/s of copying, so re-sending up to this many *unchanged* rows (they are the
/// live rows, straight out of `rows`; nothing stale can be written) is cheaper than a second
/// call. Sized so the whole tolerance costs about what one call does. The case that needed it:
/// a 40-man raid at the Stormwind auction house uploaded ~310 KB of rows in ~93 calls a frame
/// (1929's `[rig-upload]` census), because parked bodies and idle props sit between the live
/// rigs in the slab and a strictly-adjacent merge stops at every one of them.
const COALESCE_GAP_BONES: u32 = 256;

/// Sort + merge overlapping, adjacent, and *nearly* adjacent (gap ≤ [`COALESCE_GAP_BONES`])
/// `(base, len)` bone ranges into maximal runs. Rows inside a bridged gap are written with their
/// current contents.
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

/// `WOW_RIG_CENSUS=<secs>` (any unparseable value = 5): a periodic one-line breakdown of WHO
/// holds the palette's slots — per-lane slots(bones) with the doodad lane split live/parked by
/// its draw gate, a bones-per-slot histogram, denials since the last line, and the prop-probe
/// table's live/peak. The last matters because probes and rigs are the two `MeshTag`-slot-indexed
/// tables competing for the same 30 payload bits — a re-split of either field must read both
/// occupancies (decision 0863's exhaustion diagnosis). Zero-cost when off: one env read, once.
fn rig_census_every() -> Option<f32> {
    static EVERY: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    *EVERY.get_or_init(|| {
        std::env::var("WOW_RIG_CENSUS")
            .ok()
            .map(|v| v.parse().unwrap_or(5.0))
    })
}

/// The `[rig-census]` printer (`WOW_RIG_CENSUS=<secs>`) — see [`rig_census_every`].
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
    // Lanes: doodad anim hosts (split by their draw gate), net units, everything else (held-item
    // billboard rigs, spell-fx instances, booth rigs). `(slots, bones)` each.
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
                // After the last joint-world writer: propagation, then the billboard joint pass
                // (which rewrites joint GlobalTransforms in place — see its module doc).
                .after(crate::billboard::BillboardPlace),
        );
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render.add_systems(
            Render,
            upload_rig_palettes.in_set(RenderSystems::PrepareResources),
        );
    }
}

/// Spawn one entity per bone of a model's rest skeleton, parented into the bone tree, each
/// carrying the [`RigJoint`] back-link to the rig root whose palette it fills.
///
/// Engine work that lived in `entities::attach` because the unit spawner was the first lane to
/// want it — and the doodad animator, the portrait booth, the equipment spawner and the spell-fx
/// spawner all reached across for it since. Its only cross-module reference was `RigJoint`, which
/// is here, so this is where it goes. The lanes have been leaving one by one for the collapsed
/// `RigPose` buffer (units 0724, doodads 1365, booths 1443); what remains is the **attached
/// model** lanes — equipment and spell-fx, whose joints hang under someone else's bone.
///
/// `holder` is the rig root the palette belongs to; `root` is what an unparented joint attaches
/// to. They differ for an attached model whose joints hang under someone else's bone.
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
                // Visibility too, not just Transform: held items and spell effects hang their
                // visible roots under joints, and a gap in the chain both trips Bevy's B0004 and
                // orphans those subtrees from the unit root's visibility (a hidden unit would
                // keep its weapon on screen). Chain only — a joint renders nothing
                // (`crate::vis_chain`).
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
        // Free the middle range: the free list holds it separately…
        p.free(s2);
        assert_eq!(p.occupancy().0, 2);
        // …an exact-fit alloc reuses it…
        let (s4, b4) = p.alloc(20).unwrap();
        assert_eq!(b4, 10);
        // …and freeing everything coalesces back to one range.
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
        // The free is a dirty range too — the GPU mirror zeroes with us.
        assert!(p.dirty.contains(&(base, 2, false)));
    }

    /// The COW clone copies the live prefix and nothing else: a shared-Arc write
    /// used to `Arc::make_mut` all 3×131072 rows — ~6.3 MB, once per frame, wherever any rig
    /// wrote — where the bone watermark bounds the content. The invariant it leans on is pinned
    /// here too: rows at or above the watermark are bitwise zero, through alloc, write, free and
    /// coalesce — including free()'s own ordering (watermark before the range re-enters the
    /// list, so the zeroing lands inside the copied prefix).
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
        // The extract's held reference — the shared state every first-write-of-a-frame sees.
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
        // Free the top rig while shared again: the zeroing must land inside the copied prefix.
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

    /// **The rider write's two contracts**: one frame reaches EVERY row of the
    /// slot — which is what makes an attached model's bind-pose palette equal its static
    /// placement — and re-writing the same frame is a no-op, so a standing unit's five
    /// attachments cost five compares a frame and never an upload.
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

        // Every row carries the same frame: a bind-pose model's `world_from_joint × ibp` is the
        // placement itself for every joint, so the whole model is one matrix repeated. Read back
        // through the picker's own reconstruction, which is the only public row reader.
        let pal = p.world_palette(skin.slot, 4).unwrap();
        let want = Mat4::from(Affine3A::from_translation(origin) * frame);
        for (b, m) in pal.iter().enumerate() {
            assert!(m.abs_diff_eq(want, 1e-2), "row {b}: {m:?} vs {want:?}");
        }
        // And the unsummed pair the vertex stage actually reads.
        let (o, t) = p.rider_placement(skin.slot).unwrap();
        assert_eq!(o, origin);
        assert!(t.abs_diff_eq(Vec3::from(frame.translation), 1e-6));

        // Idempotent: the standing-still case must not reach the upload.
        p.dirty.clear();
        p.write_rider(skin.slot, frame, origin);
        assert!(
            p.dirty.is_empty(),
            "an unchanged rider frame must not dirty its range"
        );
        // A real move still lands — bit equality, not an epsilon (the 1362 gate's rule).
        p.write_rider(
            skin.slot,
            Affine3A::from_translation(Vec3::new(0.3, 1.4, -0.2 + 1.0e-6)) * frame,
            origin,
        );
        assert_eq!(p.dirty.len(), 1, "a sub-millimetre move is still a move");
    }

    /// The picker's read side puts the slot's ORIGIN back on — rows are stored
    /// rig-relative, so a reconstruction that forgot the origin would ray-test every skinned unit
    /// against a copy of itself sitting at the map origin.
    #[test]
    fn world_palette_reconstructs_the_affine_including_the_rig_origin() {
        let mut p = RigPalettes::default();
        let (slot, base) = p.alloc(1).unwrap();
        // A recognisable affine: rotate 90° about Y, translate (1,2,3) — inside a rig standing a
        // long way from the map origin.
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
        // A transformed point round-trips through the reconstruction.
        let q = Vec3::new(0.5, -1.0, 2.0);
        assert!(pal[0]
            .transform_point3(q)
            .abs_diff_eq(want.transform_point3(q), 1e-2));
        // And the origin is really load-bearing here, not a rounding-sized detail.
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
        // Adjacent + overlapping merge; a hole splits. Order-independent (upload sorts).
        assert_eq!(
            coalesce_ranges_with_gap(vec![(70, 5), (0, 10), (10, 20), (25, 10), (40, 5)], 0),
            vec![(0, 35), (40, 5), (70, 5)]
        );
        assert_eq!(coalesce_ranges(Vec::new()), Vec::new());
    }

    #[test]
    fn dirty_ranges_bridge_small_gaps_but_not_large_ones() {
        // A parked rig's rows between two live ones (the raid's ~93-call frame): bridged, so
        // the run is one call. A gap wider than the tolerance still splits.
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
        // The slot table sits after the probe region, then the per-instance tint table (decision
        // 0812 — same slot index), then the rig-origin table (same slot index
        // again), the palette rows last, and the blob-size extension covers all four exactly
        // (wow_model.wgsl mirrors this layout, and wgpu validates the bound size against the
        // shader's struct at draw time — a mismatch here is a draw-time failure, which is why this
        // is asserted rather than trusted).
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
        // One `vec4` per addressable slot — the shader declares `array<vec4<f32>, 2048>`.
        assert_eq!(rig_origin_region_bytes(), (MAX_RIG_SLOTS * 16) as u64);
        // One word per addressable slot — the shader declares `array<u32, 2048>` for both tables.
        assert_eq!(
            crate::instance_tint::region_bytes(),
            (MAX_RIG_SLOTS * 4) as u64
        );
    }
}
