//! Off-thread static-collider builds for streamed geometry (terrain tiles, doodads, WMOs,
//! props). parry's trimesh/QBVH construction is the hundreds-of-ms cost that hitched the frame
//! on stream-in, so the build runs on the async compute pool ([`build_collider_task`]) and
//! [`finish_colliders`] attaches each result under a per-frame budget.
//!
//! The build being off-thread is only half the story: *attaching* the finished shape is main-thread
//! structural work (an archetype move plus avian's required components and its on-insert
//! mass-property hook), and the async pool finishes a streamed burst together — so a tile boundary
//! used to attach the whole burst in a single frame. Decision 0610 measured that path and caps it.

use std::time::{Duration, Instant};

use avian3d::prelude::{Collider, CollisionLayers, RigidBody};
use benilla_assets::coords::wow_to_bevy;
use benilla_formats::CollisionMesh;
use bevy::ecs::system::SystemState;
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{block_on, AsyncComputeTaskPool, Task};

/// Wall-clock spent per frame *attaching* finished colliders before deferring the rest to a later
/// frame — the collider twin of `SPAWN_BUDGET`, and for the same reason. Measured cost of one attach
/// (decision 0610): ~0.004 ms per entity plus ~1.8e-5 ms per triangle, so a burst of a thousand-odd
/// doodads (or a dozen tiles) landing together is the 9.45 ms of main-thread time the traced flight
/// hitch spent here. Deferring is nearly free to *feel*: the streamer loads ahead of the view, so
/// just-streamed static geometry going solid a few frames late is not yet reachable.
const ATTACH_BUDGET: Duration = Duration::from_millis(2);

/// The attach budget as a resource, so a test can hold it still (2331). Absent — always, in the
/// client — [`finish_colliders`] spends [`ATTACH_BUDGET`] of wall clock; present, it spends this.
/// `Duration::ZERO` is the fully deterministic setting: exactly one attach per call, because the
/// deadline is checked *after* each attach and a zero budget is already past.
#[derive(Resource)]
pub(super) struct AttachBudget(pub(super) Duration);

/// A static collider being built on the async compute pool (see [`build_collider_task`]); once the
/// parry shape is ready, [`finish_colliders`] inserts it (with `layers`, for the WMO walk/camera
/// audiences) onto this entity. Building off-thread keeps a big-WMO / terrain stream-in from hitching
/// the frame.
#[derive(Component)]
pub struct PendingCollider {
    /// The in-flight parry build; taken the frame it completes.
    task: Option<Task<Collider>>,
    /// The finished shape, parked here when [`ATTACH_BUDGET`] deferred its attach to a later frame.
    /// A completed [`Task`] cannot be polled twice, so the shape needs somewhere to live between
    /// "built" and "attached" — this is that somewhere, and it is why no finished build is dropped.
    built: Option<Collider>,
    /// WMO walk / camera collision layer; `None` = the default layer (terrain, doodads, props).
    layers: Option<CollisionLayers>,
    /// Insert `RigidBody::Static` with the collider — every world-anchored placement. `false` = a
    /// bare collider that attaches to its nearest `RigidBody` ancestor: a transport deck prop's
    /// hull riding the boat's kinematic body ([`crate::entities`]' wmo_props). A `Static` body
    /// there would anchor the hull to the world at its spawn pose and the cargo would sail away
    /// from its own collision.
    static_body: bool,
}

impl PendingCollider {
    /// A collider build in flight on the async compute pool.
    pub fn new(task: Task<Collider>, layers: Option<CollisionLayers>, static_body: bool) -> Self {
        Self {
            task: Some(task),
            built: None,
            layers,
            static_body,
        }
    }

    /// A collider that is already built — the state a completed task leaves behind — so a test
    /// can stage a whole burst as ready without a thread pool or a sleep (2331).
    #[cfg(test)]
    fn ready(collider: Collider, layers: Option<CollisionLayers>, static_body: bool) -> Self {
        Self {
            task: None,
            built: Some(collider),
            layers,
            static_body,
        }
    }
}

/// Attach off-thread-built colliders to their entities, at most [`ATTACH_BUDGET`] worth per frame.
///
/// Exclusive on purpose. The previous shape queued each attach as a deferred command, which put the
/// whole burst into one `system_commands` block with nothing able to measure or stop it; owning the
/// `&mut World` is what makes a deadline enforceable at all. Polling stays cheap — the traced cost of
/// the poll pass over the entire pending set was ~0.005 ms — so the budget guards only the inserts.
pub(super) fn finish_colliders(
    world: &mut World,
    state: &mut SystemState<Query<'static, 'static, (Entity, &mut PendingCollider)>>,
) {
    // Pass 1 — poll in-flight builds. A completed task hands its shape to `built`, so running out of
    // budget below can never drop a finished collider on the floor.
    //
    // Capped: a cold zone load queues *tens of thousands* of pending colliders (10 500 at once
    // entering Stormwind), and the budget below can attach at most ~500 of the cheapest ones in a
    // frame. Collecting every ready entity would rebuild a huge list each frame to use a slice of
    // it. `READY_SCAN_CAP` bounds the ready list while staying far above what any frame can spend;
    // the rest are picked up next frame (they stay in the query until attached, so none is skipped).
    // The walk itself covers the whole set regardless (it is the ~0.005 ms part) — it also yields
    // the queue-depth publish below.
    const READY_SCAN_CAP: usize = 2048;
    let t0 = Instant::now();
    let mut ready: Vec<Entity> = Vec::new();
    let mut pending = 0usize;
    for (entity, mut pc) in state.get_mut(world).iter_mut() {
        pending += 1;
        if let Some(task) = pc.task.as_mut() {
            if let Some(collider) = block_on(future::poll_once(task)) {
                pc.task = None;
                pc.built = Some(collider);
            }
        }
        if pc.built.is_some() && ready.len() < READY_SCAN_CAP {
            ready.push(entity);
        }
    }
    // Publish the queue depth (decision 0737): the loading-screen clear and the settle release
    // both refuse to call the world presentable while attaches are outstanding — a spawned
    // building whose collider still sits in this queue is exactly what a body must not be
    // released onto. This system heads the Stream chain (0738), so the consumers read this
    // frame's depth; the count is the depth entering the frame (attaches below shrink it next
    // publish), which can only delay a release, never wrong one.
    // The weld accumulators count as pending too (decision 1369): each unflushed batch becomes
    // at least one collider on this queue, and the settle release must not let a body go while
    // doodad hulls still sit in one. Same conservative semantics as the depth itself.
    let weld_backlog = world
        .get_resource::<super::weld::HullWelds>()
        .map_or(0, |w| w.unflushed());
    if let Some(mut progress) = world.get_resource_mut::<crate::terrain_stream::WorldLoadProgress>()
    {
        progress.colliders_pending = pending + weld_backlog;
    }
    if ready.is_empty() {
        // `get_`: the unit tests below drive this system on a minimal App without the perf layer.
        if let Some(mut activity) =
            world.get_resource_mut::<crate::terrain_stream::StreamActivity>()
        {
            activity.collider_ms += t0.elapsed().as_secs_f32() * 1000.0;
        }
        return;
    }

    // Pass 2 — the main-thread cost. The budget is checked *after* each attach so one oversized
    // collider still makes progress rather than stalling the queue forever (the rule
    // `stream_terrain` already applies to its tile spawns).
    let mut attached = 0u32;
    let budget = world
        .get_resource::<AttachBudget>()
        .map_or(ATTACH_BUDGET, |b| b.0);
    let deadline = Instant::now() + budget;
    for entity in ready {
        // The entity may have streamed out (despawned) while its collider was building, or had its
        // slot reused — a no-op rather than a panic.
        let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
            continue;
        };
        let Some(mut pc) = entity_mut.take::<PendingCollider>() else {
            continue;
        };
        let Some(collider) = pc.built.take() else {
            continue;
        };
        entity_mut.insert(collider);
        if pc.static_body {
            entity_mut.insert(RigidBody::Static);
        }
        if let Some(layers) = pc.layers {
            entity_mut.insert(layers);
        }
        attached += 1;
        if Instant::now() >= deadline {
            break;
        }
    }
    if let Some(mut activity) = world.get_resource_mut::<crate::terrain_stream::StreamActivity>() {
        activity.colliders_attached += attached;
        activity.collider_ms += t0.elapsed().as_secs_f32() * 1000.0;
    }
    // The world a cached collision answer was computed against just changed — see
    // [`crate::collision::ColliderEpoch`]. This is the attach half; removals are stamped by
    // `track_collider_removals`.
    if attached > 0 {
        if let Some(mut epoch) = world.get_resource_mut::<crate::collision::ColliderEpoch>() {
            epoch.bump();
        }
    }
}

/// `WOW_NO_DOODAD_BODIES=1` — spawn NO doodad/prop hulls at all (decision 1367's premise
/// bracket): the ceiling of what the whole lane can be worth, measured by deleting it. The
/// player loses doodad collision and the doodad pick clamp for the run — a measurement lever,
/// never a setting. Terrain, impassable fences and WMO building colliders are untouched.
pub(crate) fn doodad_bodies_disabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_NO_DOODAD_BODIES").is_some())
}

/// `WOW_BARE_DOODAD_COLLIDERS=1` — doodad/prop hulls spawn as body-LESS colliders (decision
/// 1367): avian 0.6 files a collider with no `ColliderOf` in its first-class **Standalone**
/// tree (`collider_tree/proxy_key.rs`), still spatial-queryable and still a contact target, so
/// this is the idiomatic collider-only shape itself, not an approximation of it. What the lever
/// removes is the `RigidBody::Static` row weight — the velocities/mass/solver components the
/// 1364 census counted 12.2k of at the Stormwind pin.
pub(crate) fn doodad_hulls_bare() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_BARE_DOODAD_COLLIDERS").is_some())
}

/// The world-space `(vertices, triangles)` for a model's static collider — its raw-WoW collision hull
/// mapped through `wow_to_bevy` then the placement [`Transform`] (so the collider coincides with the
/// drawn model). `None` for a hull-less model or an empty mesh. The (potentially expensive) parry build
/// is deferred off-thread via [`build_collider_task`].
pub fn placement_collider_data(
    hull: Option<&CollisionMesh>,
    transform: &Transform,
) -> Option<(Vec<Vec3>, Vec<[u32; 3]>)> {
    let hull = hull?;
    if hull.indices.len() < 3 {
        return None;
    }
    let verts: Vec<Vec3> = hull
        .positions
        .iter()
        .map(|p| transform.transform_point(wow_to_bevy(*p)))
        .collect();
    let tris: Vec<[u32; 3]> = hull
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| [c[0], c[1], c[2]])
        .collect();
    Some((verts, tris))
}

/// The `(vertices, triangles)` for a terrain tile's static collider: **one** trimesh welded from the
/// tile's decoded MCNK chunks, in the same world space they are drawn in (so you stand on the visible
/// ground). `None` for a tile with no triangles at all.
///
/// Built from the decoded chunks rather than the render meshes because the tile no longer *has* one
/// merged mesh to read (decision 0780) — and one collider per 33 yd cell would be 256 parry builds
/// and 256 QBVHs where the ground is a single continuous surface. The mapping is the loader's,
/// verbatim: `wow_to_bevy` per position, hole-masked indices rebased onto the running vertex count.
pub(super) fn terrain_collider_data(
    chunks: &[benilla_formats::ChunkMesh],
) -> Option<(Vec<Vec3>, Vec<[u32; 3]>)> {
    let mut verts: Vec<Vec3> = Vec::new();
    let mut tris: Vec<[u32; 3]> = Vec::new();
    for chunk in chunks {
        let base = verts.len() as u32;
        verts.extend(chunk.positions.iter().map(|p| wow_to_bevy(*p)));
        tris.extend(
            chunk
                .indices
                .as_chunks::<3>()
                .0
                .iter()
                .map(|c| [base + c[0], base + c[1], base + c[2]]),
        );
    }
    (!tris.is_empty()).then_some((verts, tris))
}

/// How far an impassable chunk's fence rises above the chunk's own floor — the reference's literal
/// `u = (0, 0, 32000.0)` (immediate `0x46fa0000` at `0x6ab599`, the emitter `0x6ab530`).
///
/// It is a *reach*, not a height: the quad is based at `chunk+0x4c`, the chunk AABB's **min z**, so
/// the fence stands on the chunk's lowest ground and rises 32 km. Nothing extends below — a mover
/// under the chunk floor (a mine, a tunnel, the sea bed) passes beneath the wall, and that is the
/// law, not an omission.
const FENCE_REACH: f32 = 32000.0;

/// The `(vertices, triangles)` for a tile's **impassable-chunk fences** — the ADT-level invisible
/// wall of report B129, in the same world space as the terrain collider.
///
/// The mechanism is the reference's (the emitter `0x6ab530`, reached from
/// the movement box gather `0x6721b0 → 0x6aa8b0 → 0x6aadc0`; the flag is `CMapChunk+0xc & 0x40`,
/// written word-wide at `0x6af5f0` from MCNK header bit 1, and `0x6aae2a` is its only reader in the
/// image):
///
/// - **Additive, never subtractive.** The terrain under a flagged chunk stays exactly as walkable as
///   any other; what the flag adds is a fence. Nothing is removed from the walkable set, which is
///   why a flagged chunk holds you up rather than dropping you through.
/// - **All four sides of every flagged chunk**, with no look at the neighbours. Two chunks side by
///   side each keep their own shared-side fence, so a mover already inside the band cannot cross
///   *within* it either — and the tile seam stops being a question, since nothing is ever asked
///   about a chunk in another tile.
/// - **Outward normals, so the fence is one-way.** Composed with the universal facing gate
///   (`n·dir ≤ −1e-5`, decision 0970), entry is blocked and the same face is discarded on the way
///   out: you cannot walk in, and if you start inside you can always walk out.
/// - **Whole chunk.** The flag is tested once, before the reference's 8×8 cell loop opens — unlike
///   holes and the no-doodad bits, which are per cell.
/// - `n.z == 0` on all four, so a fence is never a floor and never something to step up onto.
///
/// The audience is the other half of the law and lives at the call site: the segment/ray path never
/// reads the flag, so camera pull-in, mouse-pick and LOS are wall-blind, and terrain *height* is not
/// gated at all.
pub(super) fn impassable_wall_data(
    chunks: &[benilla_formats::ChunkMesh],
) -> Option<(Vec<Vec3>, Vec<[u32; 3]>)> {
    let mut verts: Vec<Vec3> = Vec::new();
    let mut tris: Vec<[u32; 3]> = Vec::new();
    for chunk in chunks.iter().filter(|c| c.impassable) {
        // `chunk+0x4c`: the min corner of the AABB the reference folds over the chunk's 145 world
        // vertices (`0x6b0e50`) — the chunk's lowest ground, wherever in the chunk it happens to be.
        let Some(floor) = chunk.positions.iter().map(|p| p[2]).reduce(f32::min) else {
            continue; // a chunk with no vertices has no AABB to stand a fence on
        };
        // The four footprint sides, from the corners of the 9×9 outer grid (stride 17: row `r`
        // starts at `r·17`), each walked so that `(b−a)×(c−a)` points AWAY from the chunk. With +X
        // north, +Y west and +Z up, a side whose outward normal is `n` must be walked along
        // `t = ẑ × n`: south down the west side, north up the east side, west along the north side,
        // east along the south side.
        const SIDES: [[usize; 2]; 4] = [
            [8, 0],     // north (row 0), walked east → west
            [136, 144], // south (row 8), walked west → east
            [0, 136],   // west (col 0), walked north → south
            [144, 8],   // east (col 8), walked south → north
        ];
        for [from, to] in SIDES {
            let (Some(a), Some(b)) = (chunk.positions.get(from), chunk.positions.get(to)) else {
                continue; // a chunk without its outer grid has no side to stand a fence on
            };
            let base = verts.len() as u32;
            for (p, z) in [
                (a, floor),
                (b, floor),
                (b, floor + FENCE_REACH),
                (a, floor + FENCE_REACH),
            ] {
                verts.push(wow_to_bevy([p[0], p[1], z]));
            }
            tris.push([base, base + 1, base + 2]);
            tris.push([base, base + 2, base + 3]);
        }
    }
    (!tris.is_empty()).then_some((verts, tris))
}

/// Build a static-trimesh [`Collider`] on the **async compute pool** (parry's QBVH construction is the
/// hundreds-of-ms cost that hitched the frame on big-WMO / terrain stream-in). [`finish_colliders`]
/// attaches the result. A few-frame delay in collision for just-streamed *static* geometry is
/// imperceptible — the streamer loads ahead of the view, so you never reach it before it's solid.
pub fn build_collider_task(verts: Vec<Vec3>, tris: Vec<[u32; 3]>) -> Task<Collider> {
    AsyncComputeTaskPool::get().spawn(async move { Collider::trimesh(verts, tris) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use avian3d::prelude::PhysicsPlugins;
    use bevy::app::TaskPoolPlugin;
    use bevy::ecs::system::RunSystemOnce;

    use benilla_formats::{ChunkMesh, CHUNK_SIZE};

    /// One MCNK at tile-grid `(index_x, index_y)`, flat at z = 0, carrying only what the wall
    /// builder reads: the 9×9 outer grid in its stride-17 slots, the indices, and the flag. The
    /// tile's NW corner sits at the WoW origin, so chunk `(ix, iy)` runs from `(−iy·C, −ix·C)`
    /// south and east — rows step −x, columns step −y, exactly as `adt_to_tile_mesh` lays them out.
    fn chunk(ix: u32, iy: u32, impassable: bool) -> ChunkMesh {
        let (nw_x, nw_y) = (-(iy as f32) * CHUNK_SIZE, -(ix as f32) * CHUNK_SIZE);
        let mut positions = vec![[0.0_f32; 3]; 145];
        for r in 0..9usize {
            for c in 0..9usize {
                positions[r * 17 + c] = [
                    nw_x - r as f32 * CHUNK_SIZE / 8.0,
                    nw_y - c as f32 * CHUNK_SIZE / 8.0,
                    0.0,
                ];
            }
        }
        ChunkMesh {
            positions,
            normals: Vec::new(),
            uvs: Vec::new(),
            indices: Vec::new(),
            holes: 0,
            base_texture: None,
            layer_textures: Vec::new(),
            layer_effect_ids: Vec::new(),
            alpha_map: None,
            shadow: None,
            pred_tex: [0; 64],
            no_effect_doodad: [false; 64],
            index_x: ix,
            index_y: iy,
            area_id: 0,
            impassable,
            liquids: Vec::new(),
        }
    }

    /// The centre of chunk `(ix, iy)`'s footprint in Bevy space, at z = 0.
    fn chunk_centre(ix: u32, iy: u32) -> Vec3 {
        wow_to_bevy([
            -(iy as f32 + 0.5) * CHUNK_SIZE,
            -(ix as f32 + 0.5) * CHUNK_SIZE,
            0.0,
        ])
    }

    /// A flagged chunk is boxed in, and every face is wound OUTWARD — which is the whole mechanism:
    /// composed with the one-sided movement law (0970), the fence blocks a mover walking in and is
    /// discarded from behind, so the band cannot be entered and never traps what starts inside it.
    #[test]
    fn a_flagged_chunk_is_boxed_in_by_outward_facing_walls() {
        let chunks: Vec<ChunkMesh> = [(1, 1, true), (0, 1, false), (1, 0, false)]
            .into_iter()
            .map(|(ix, iy, f)| chunk(ix, iy, f))
            .collect();
        let (verts, tris) = impassable_wall_data(&chunks).expect("one flagged chunk, four walls");
        assert_eq!(tris.len(), 8, "four sides, two triangles each");

        let centre = chunk_centre(1, 1);
        for t in &tris {
            let (a, b, c) = (
                verts[t[0] as usize],
                verts[t[1] as usize],
                verts[t[2] as usize],
            );
            let n = (b - a).cross(c - a).normalize();
            assert!(n.y.abs() < 1e-5, "a wall face is vertical: {n:?}");
            let face = (a + b + c) / 3.0;
            assert!(
                n.dot(face - centre) > 0.0,
                "face at {face:?} is wound back INTO the chunk (normal {n:?})"
            );
        }
    }

    /// Every flagged chunk keeps its **own** four sides, neighbours unread — so two side by side
    /// carry a doubled fence on the boundary they share, wound apart. That is not redundancy: it is
    /// what stops a mover already inside the band from crossing *within* it, since each chunk's
    /// fence blocks entry to that chunk from any direction. It also makes the tile seam a non-issue,
    /// because nothing is ever asked about a chunk in another tile — the case B129 is (the flagged
    /// chunk is column 0 of `Azeroth_33_44`, the pin is column 15 of `Azeroth_32_44`).
    #[test]
    fn every_flagged_chunk_fences_all_four_of_its_own_sides() {
        let chunks: Vec<ChunkMesh> = [(1, 1, true), (2, 1, true), (0, 1, false), (3, 1, false)]
            .into_iter()
            .map(|(ix, iy, f)| chunk(ix, iy, f))
            .collect();
        let (verts, tris) = impassable_wall_data(&chunks).expect("two flagged chunks");
        assert_eq!(tris.len(), 16, "four sides each, two triangles a side");

        // The shared plane is WoW y = −2·CHUNK_SIZE ⇒ Bevy x = +2·CHUNK_SIZE. Both chunks put a
        // face IN it, and the two must face opposite ways — each blocking entry to its own chunk.
        let shared = 2.0 * CHUNK_SIZE;
        let normals: Vec<Vec3> = tris
            .iter()
            .filter(|t| {
                t.iter()
                    .all(|&i| (verts[i as usize].x - shared).abs() < 1e-3)
            })
            .map(|t| {
                let (a, b, c) = (
                    verts[t[0] as usize],
                    verts[t[1] as usize],
                    verts[t[2] as usize],
                );
                (b - a).cross(c - a).normalize()
            })
            .collect();
        assert_eq!(normals.len(), 4, "two coplanar quads on the shared side");
        assert!(
            normals.iter().any(|n| n.x > 0.9) && normals.iter().any(|n| n.x < -0.9),
            "the shared side is fenced from both directions: {normals:?}"
        );
    }

    /// A fence stands on the chunk's own floor and only ever rises — the reference bases the quad
    /// at the chunk AABB's min z and adds `(0, 0, 32000)`. Nothing below: a mover under the chunk
    /// floor (a mine, a tunnel, the sea bed) passes beneath the wall, and that is the law.
    #[test]
    fn a_fence_stands_on_the_chunk_floor_and_only_rises() {
        // A chunk dished 12 yd below its corners, so "the floor" is a real minimum and not just the
        // corner height the sides are built from.
        let mut c = chunk(1, 1, true);
        c.positions[4 * 17 + 4][2] = -12.0;
        let (verts, _) = impassable_wall_data(&[c]).expect("walls");
        let (lo, hi) = verts.iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| {
            (lo.min(v.y), hi.max(v.y))
        });
        assert!(
            (lo - -12.0).abs() < 1e-3,
            "based at the chunk floor, got {lo}"
        );
        assert!(
            (hi - (-12.0 + FENCE_REACH)).abs() < 1e-3,
            "rises {FENCE_REACH} from the floor, got {hi}"
        );
    }

    /// The overwhelmingly common tile: no flag, no collider, nothing attached.
    #[test]
    fn a_tile_with_no_flagged_chunk_builds_no_wall() {
        let chunks = vec![chunk(0, 0, false), chunk(1, 0, false)];
        assert!(impassable_wall_data(&chunks).is_none());
    }

    /// B129 end to end, on the shipped bytes and through the real cast: a body walking east from
    /// the pin (`.go xyz -6601.98 -531.87 335.60 0`) is stopped where 1.12.1 stops it, at the
    /// chunk boundary 1.46 yd away — and the SAME world built without the walls carries it straight
    /// through, which is the report. Both halves matter: the second is the symptom reproduced, the
    /// first is it gone, and a wall built at the wrong offset would satisfy neither. Skips without
    /// client data.
    #[test]
    fn a_body_walking_east_from_the_b129_pin_is_stopped_at_the_wall() {
        /// The reported pin, and the MCNK boundary the flagged chunk starts at (WoW y; east is −y).
        const PIN: [f32; 3] = [-6601.98, -531.87, 335.60];
        const WALL_Y: f32 = 32.0 * benilla_formats::TILE_SIZE - 528.0 * CHUNK_SIZE;
        const R: f32 = 0.5;
        const SKIN: f32 = 0.01;

        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        // Both sides of the seam: the pin's tile and the one that authors the wall.
        let tiles: Vec<benilla_formats::TileMesh> = [(32, 44), (33, 44)]
            .into_iter()
            .map(|(tx, ty)| {
                benilla_formats::load_tile_mesh(&mut chain, "Azeroth", tx, ty)
                    .expect("Azeroth tile")
            })
            .collect();
        let ground = tiles
            .iter()
            .find_map(|t| benilla_formats::terrain_height_at(&t.chunks, PIN))
            .expect("terrain under the pin");

        // How far the body gets walking east from a given height, in a world with the walls and in
        // one without.
        let run_east = |walls: bool, z: f32| -> Option<f32> {
            let mut app = App::new();
            // `WorldCollision` takes the mover's trace exclusions (the ghost/DOOR set, 1767), which the
            // world plugins own and a bare harness does not run.
            app.init_resource::<crate::collision::MoverTraceExclusions>();
            app.add_plugins((
                MinimalPlugins,
                bevy::transform::TransformPlugin,
                bevy::asset::AssetPlugin::default(),
                bevy::scene::ScenePlugin,
                avian3d::prelude::PhysicsPlugins::new(bevy::app::PostUpdate),
            ));
            app.init_asset::<Mesh>();
            for t in &tiles {
                if let Some((v, i)) = terrain_collider_data(&t.chunks) {
                    app.world_mut().spawn((
                        RigidBody::Static,
                        Collider::trimesh(v, i),
                        Transform::default(),
                    ));
                }
                if let Some((v, i)) = walls.then(|| impassable_wall_data(&t.chunks)).flatten() {
                    app.world_mut().spawn((
                        RigidBody::Static,
                        Collider::trimesh(v, i),
                        crate::collision::walk_layers(),
                        Transform::default(),
                    ));
                }
            }
            app.update(); // builds Position/Rotation and the spatial-query trees
            app.world_mut()
                .run_system_once(move |world: crate::collision::WorldCollision| {
                    let capsule = Collider::capsule(R, 1.0);
                    // East is −y in WoW, +x in Bevy.
                    let from = wow_to_bevy([PIN[0], PIN[1], z]);
                    world
                        .cast_body(&capsule, from, Vec3::X * 5.0, SKIN)
                        .map(|h| h.distance)
                })
                .unwrap()
        };

        // Feet on the ground at the pin.
        let walking = ground + 1.0 + R;
        // The report: nothing at all stops the body inside the 5 yd it walks.
        assert!(
            run_east(false, walking).is_none(),
            "B129 itself — the terrain here does not stop a body, which is why the flag has to"
        );
        // The fix: stopped, with the capsule's leading surface against the chunk boundary.
        let d = run_east(true, walking).expect("the impassable chunk's fence stops the body");
        let leading_edge = -(wow_to_bevy([PIN[0], PIN[1], 0.0]).x + d) - R;
        assert!(
            (leading_edge - (WALL_Y + SKIN)).abs() < 0.05,
            "stopped at y={leading_edge}, wall is at y={WALL_Y} (travelled {d} yd)"
        );

        // …and the other half of the same law: the fence is based at the flagged chunk's own floor
        // and only rises, so a body BELOW that floor — a tunnel, a mine, the sea bed — crosses
        // freely. Under the ground and out of the terrain's way, the only thing that could stop
        // this cast is a fence reaching down, and the reference's does not.
        let floor = tiles
            .iter()
            .flat_map(|t| t.chunks.iter())
            .filter(|c| c.impassable)
            .filter_map(|c| c.positions.iter().map(|p| p[2]).reduce(f32::min))
            .fold(f32::MAX, f32::min);
        assert!(
            run_east(true, floor - 20.0).is_none(),
            "the fence reached below the chunk floor (floor {floor})"
        );
    }

    /// A tile-sized grid: 256 quads -> 512 triangles per chunk, `chunks` chunks.
    fn grid(chunks: usize) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let (mut verts, mut tris) = (Vec::new(), Vec::new());
        for c in 0..chunks {
            let base = verts.len() as u32;
            for r in 0..17u32 {
                for q in 0..17u32 {
                    verts.push(Vec3::new(
                        q as f32,
                        ((q + r + c as u32) % 5) as f32,
                        r as f32,
                    ));
                }
            }
            for r in 0..16u32 {
                for q in 0..16u32 {
                    let i = base + r * 17 + q;
                    tris.push([i, i + 1, i + 17]);
                    tris.push([i + 1, i + 18, i + 17]);
                }
            }
        }
        (verts, tris)
    }

    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ));
        app.init_asset::<Mesh>();
        // Registers avian's collider hooks + required components, so an attach here costs what it
        // costs in the client. Its schedules are never run — `run_system_once` drives ours alone.
        app.add_plugins(PhysicsPlugins::default());
        app.finish();
        app.cleanup();
        app
    }

    /// The other half of the queue — pass 1: a task the pool has finished hands its shape to
    /// `built` on the poll and attaches on the same call. Polled until done rather than slept for:
    /// the outcome is the pool's, the timing is not asserted.
    #[test]
    fn a_finished_task_hands_its_shape_over_and_attaches() {
        let mut app = test_app();
        let (verts, tris) = grid(4);
        let task = build_collider_task(verts, tris);
        let e = app
            .world_mut()
            .spawn((Transform::default(), PendingCollider::new(task, None, true)))
            .id();
        for _ in 0..100_000 {
            app.world_mut().run_system_once(finish_colliders).unwrap();
            if app.world().entity(e).contains::<Collider>() {
                break;
            }
            std::thread::yield_now();
        }
        assert!(
            app.world().entity(e).contains::<Collider>(),
            "the pool finished the build (or never did — a hung pool, not a budget)"
        );
        assert!(
            app.world().entity(e).contains::<RigidBody>(),
            "a static body rides the attach"
        );
        assert!(app.world().get::<PendingCollider>(e).is_none());
    }

    /// The budget must both bite (a burst is spread over frames) and never lose a finished build.
    /// The second half is the subtle one: a completed `Task` cannot be polled twice, so a deferred
    /// attach has to park its shape rather than leave it in the task.
    ///
    /// Deterministic since 2331: the burst is staged already-built (`PendingCollider::ready`) and
    /// the budget is held at zero through [`AttachBudget`], so one call attaches exactly one. The
    /// earlier shape slept 500 ms for a real pool and then asserted that forty `grid(16)` attaches
    /// outran a 2 ms wall clock — true on the machine it was written on, false on a faster one,
    /// and false the other way on a starved pool.
    #[test]
    fn attach_budget_spreads_a_burst_without_losing_colliders() {
        const N: usize = 40;
        let mut app = test_app();
        app.insert_resource(AttachBudget(std::time::Duration::ZERO));
        let (verts, tris) = grid(16);
        let entities: Vec<Entity> = (0..N)
            .map(|_| {
                let collider = Collider::trimesh(verts.clone(), tris.clone());
                app.world_mut()
                    .spawn((
                        Transform::default(),
                        PendingCollider::ready(collider, None, true),
                    ))
                    .id()
            })
            .collect();

        let attached = |app: &mut App| {
            entities
                .iter()
                .filter(|e| app.world().entity(**e).contains::<Collider>())
                .count()
        };

        app.world_mut().run_system_once(finish_colliders).unwrap();
        assert_eq!(
            attached(&mut app),
            1,
            "a zero budget is past after the first attach: exactly one lands per call"
        );

        // Drain: every remaining build must still land, none lost to the deferral.
        for _ in 0..N {
            if attached(&mut app) == N {
                break;
            }
            app.world_mut().run_system_once(finish_colliders).unwrap();
        }
        assert_eq!(attached(&mut app), N, "a finished collider was dropped");
        for e in &entities {
            assert!(app.world().entity(*e).contains::<RigidBody>());
            assert!(!app.world().entity(*e).contains::<PendingCollider>());
        }
    }
}
