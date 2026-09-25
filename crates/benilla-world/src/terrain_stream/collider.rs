//! Static colliders for streamed geometry: parry's trimesh build runs on the async compute pool
//! ([`build_collider_task`]) and [`finish_colliders`] attaches the results under a per-frame
//! budget, since a streamed burst finishes together and each attach is main-thread work.

use std::time::{Duration, Instant};

use avian3d::prelude::{Collider, CollisionLayers, RigidBody};
use benilla_assets::coords::wow_to_bevy;
use benilla_formats::CollisionMesh;
use bevy::ecs::system::SystemState;
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{block_on, AsyncComputeTaskPool, Task};

/// Wall clock per frame for attaching finished colliders (one costs ~0.004 ms plus ~1.8e-5 ms per
/// triangle); the rest wait, and the streamer loads far enough ahead that none is reachable yet.
const ATTACH_BUDGET: Duration = Duration::from_millis(2);

/// A test's override of [`ATTACH_BUDGET`]; `Duration::ZERO` attaches exactly one per call.
#[derive(Resource)]
pub(super) struct AttachBudget(pub(super) Duration);

/// A static collider building on the async compute pool, which [`finish_colliders`] attaches to
/// this entity.
#[derive(Component)]
pub struct PendingCollider {
    /// The in-flight parry build; taken the frame it completes.
    task: Option<Task<Collider>>,
    /// The finished shape, waiting out a deferred attach: a completed [`Task`] cannot be polled
    /// twice.
    built: Option<Collider>,
    /// WMO walk / camera collision layer; `None` = the default layer (terrain, doodads, props).
    layers: Option<CollisionLayers>,
    /// Insert `RigidBody::Static` too. `false` for a transport deck prop, whose bare collider
    /// rides the boat's kinematic body; a static one would stay at its spawn pose.
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

    /// An already-built collider, so a test stages a burst without a thread pool.
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

/// Attaches off-thread-built colliders, at most [`ATTACH_BUDGET`] worth per frame. Exclusive, so
/// the deadline can stop mid-burst where deferred commands would all apply in one block.
pub(super) fn finish_colliders(
    world: &mut World,
    state: &mut SystemState<Query<'static, 'static, (Entity, &mut PendingCollider)>>,
) {
    // Pass 1: poll every in-flight build; a completed task hands its shape to `built`. The ready
    // list is capped far above what a frame can attach (a cold zone load queues tens of
    // thousands); the rest stay queued for the next frame.
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
    // The queue depth, unflushed welds included: the loading screen and the settle release wait
    // on it, so no body is released onto geometry without its collider. An overcount only delays.
    let weld_backlog = world
        .get_resource::<super::weld::HullWelds>()
        .map_or(0, |w| w.unflushed());
    if let Some(mut progress) = world.get_resource_mut::<crate::terrain_stream::WorldLoadProgress>()
    {
        progress.colliders_pending = pending + weld_backlog;
    }
    if ready.is_empty() {
        // `get_`: the unit tests run without the perf layer.
        if let Some(mut activity) =
            world.get_resource_mut::<crate::terrain_stream::StreamActivity>()
        {
            activity.collider_ms += t0.elapsed().as_secs_f32() * 1000.0;
        }
        return;
    }

    // Pass 2: attach. The deadline is checked after each attach, so an oversized one still lands.
    let mut attached = 0u32;
    let budget = world
        .get_resource::<AttachBudget>()
        .map_or(ATTACH_BUDGET, |b| b.0);
    let deadline = Instant::now() + budget;
    for entity in ready {
        // The entity may have streamed out while its collider was building.
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
    // Invalidates cached collision answers (`crate::collision::ColliderEpoch`).
    if attached > 0 {
        if let Some(mut epoch) = world.get_resource_mut::<crate::collision::ColliderEpoch>() {
            epoch.bump();
        }
    }
}

/// `WOW_NO_DOODAD_BODIES=1` spawns no doodad or prop hulls: a measurement lever that drops their
/// collision and pick clamp for the run.
pub(crate) fn doodad_bodies_disabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_NO_DOODAD_BODIES").is_some())
}

/// `WOW_BARE_DOODAD_COLLIDERS=1` spawns doodad and prop hulls without `RigidBody::Static`: avian
/// files a body-less collider as standalone, still spatial-queryable and a contact target.
pub(crate) fn doodad_hulls_bare() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_BARE_DOODAD_COLLIDERS").is_some())
}

/// A model's collision hull in world space as `(vertices, triangles)`: `wow_to_bevy`, then the
/// placement [`Transform`].
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

/// A terrain tile's collider: one trimesh welded from its decoded MCNK chunks in draw space, the
/// loader's hole-masked indices rebased onto the running vertex count.
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

/// How far an impassable chunk's fence rises from the chunk AABB's min z (`chunk+0x4c`): the
/// reference's `(0, 0, 32000.0)` (`0x46fa0000` at `0x6ab599`, emitter `0x6ab530`). Nothing extends
/// below, so a mover under the chunk floor (a mine, the sea bed) passes beneath.
const FENCE_REACH: f32 = 32000.0;

/// A tile's impassable-chunk fences as `(vertices, triangles)`, the reference's emitter
/// `0x6ab530` reached from the movement box gather `0x6721b0 → 0x6aa8b0 → 0x6aadc0`. The flag is
/// `CMapChunk+0xc & 0x40` (set at `0x6af5f0` from MCNK header bit 1, read only at `0x6aae2a`),
/// tested once per chunk.
///
/// - Additive: the terrain under a flagged chunk stays walkable; the flag only adds a fence.
/// - All four sides of every flagged chunk, neighbours unread, so a mover inside the band cannot
///   cross within it either and no tile seam is ever consulted.
/// - Outward normals: with the facing gate (`n·dir ≤ −1e-5`) you cannot walk in, and can always
///   walk out.
/// - `n.z == 0`: a fence is never a floor or a step.
///
/// The segment and ray path never reads the flag: camera pull-in, mouse-pick and LOS are
/// wall-blind, and terrain height is not gated.
pub(super) fn impassable_wall_data(
    chunks: &[benilla_formats::ChunkMesh],
) -> Option<(Vec<Vec3>, Vec<[u32; 3]>)> {
    let mut verts: Vec<Vec3> = Vec::new();
    let mut tris: Vec<[u32; 3]> = Vec::new();
    for chunk in chunks.iter().filter(|c| c.impassable) {
        // `chunk+0x4c`: the min corner of the AABB over the chunk's 145 vertices (`0x6b0e50`).
        let Some(floor) = chunk.positions.iter().map(|p| p[2]).reduce(f32::min) else {
            continue; // a chunk with no vertices has no AABB to stand a fence on
        };
        // The four sides from the 9×9 outer grid's corners (row `r` starts at `r·17`), each walked
        // along `t = ẑ × n` (+X north, +Y west) so `(b−a)×(c−a)` points out of the chunk.
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

/// Builds a static trimesh [`Collider`] on the async compute pool, for [`finish_colliders`].
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

    /// One flat MCNK at `(ix, iy)` with only what the wall builder reads. The tile's NW corner is
    /// the WoW origin: rows step −x and columns −y, as `adt_to_tile_mesh` lays them out.
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

    /// Two flagged neighbours carry a doubled fence on their shared side, wound apart.
    #[test]
    fn every_flagged_chunk_fences_all_four_of_its_own_sides() {
        let chunks: Vec<ChunkMesh> = [(1, 1, true), (2, 1, true), (0, 1, false), (3, 1, false)]
            .into_iter()
            .map(|(ix, iy, f)| chunk(ix, iy, f))
            .collect();
        let (verts, tris) = impassable_wall_data(&chunks).expect("two flagged chunks");
        assert_eq!(tris.len(), 16, "four sides each, two triangles a side");

        // The shared plane: WoW y = −2·CHUNK_SIZE, Bevy x = +2·CHUNK_SIZE.
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

    #[test]
    fn a_fence_stands_on_the_chunk_floor_and_only_rises() {
        // Dished 12 yd below its corners, so the floor is not the corners' height.
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

    #[test]
    fn a_tile_with_no_flagged_chunk_builds_no_wall() {
        let chunks = vec![chunk(0, 0, false), chunk(1, 0, false)];
        assert!(impassable_wall_data(&chunks).is_none());
    }

    /// On the shipped data, through the real cast: a body walking east from the pin stops at the
    /// flagged chunk's boundary 1.46 yd away, as 1.12.1 stops it, and walks through without walls.
    #[test]
    fn a_body_walking_east_from_the_pin_is_stopped_at_the_wall() {
        /// The pin, and the MCNK boundary the flagged chunk starts at (WoW y; east is −y).
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

        // How far a body walks east from height `z`, with or without the walls.
        let run_east = |walls: bool, z: f32| -> Option<f32> {
            let mut app = App::new();
            // `WorldCollision` reads the mover's trace exclusions, which the world plugins own.
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
        // Without the walls nothing stops the body within the 5 yd it walks.
        assert!(
            run_east(false, walking).is_none(),
            "the pin itself — the terrain here does not stop a body, which is why the flag has to"
        );
        // With them, the capsule's leading surface stops at the chunk boundary.
        let d = run_east(true, walking).expect("the impassable chunk's fence stops the body");
        let leading_edge = -(wow_to_bevy([PIN[0], PIN[1], 0.0]).x + d) - R;
        assert!(
            (leading_edge - (WALL_Y + SKIN)).abs() < 0.05,
            "stopped at y={leading_edge}, wall is at y={WALL_Y} (travelled {d} yd)"
        );

        // Below the flagged chunk's floor only a fence reaching down could stop the body, and the
        // reference's does not.
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
        // Avian's collider hooks, so an attach costs what it does in the client; its schedules
        // never run.
        app.add_plugins(PhysicsPlugins::default());
        app.finish();
        app.cleanup();
        app
    }

    /// Polled until done, not slept for: the timing is the pool's and is not asserted.
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

    /// Staged already built with a zero budget, so each call attaches exactly one.
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
