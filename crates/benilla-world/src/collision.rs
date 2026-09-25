//! World collision: the layers and traces the body and the camera query. The reference's walking
//! box query (leaf `0x6bca50`) drops MOPY DETAIL faces (`0x04`) and its camera/LOS segment query
//! (leaf `0x6bc700`) drops NOCAMCOLLIDE faces (`0x02`), so the camera hits a low beam the body
//! walks under. avian cannot filter one trimesh per face, so each WMO bakes one collider per
//! audience; terrain, doodads and GameObjects sit on the default layer, which both see.

use avian3d::character_controller::move_and_slide::{
    MoveAndSlide, MoveAndSlideConfig, MoveAndSlideHitData, MoveAndSlideHitResponse,
    MoveAndSlideOutput, MoveHitData,
};
use avian3d::prelude::*;
use bevy::ecs::component::Component;
use bevy::ecs::lifecycle::RemovedComponents;
use bevy::ecs::resource::Resource;
use bevy::ecs::system::ResMut;
use bevy::math::{Dir3, Quat, Vec3};

mod one_sided;

/// The collision audiences. avian puts every collider without explicit `CollisionLayers` on bit 0,
/// [`CollisionLayer::Default`]; only WMO faces and liquid carry a layer.
#[derive(PhysicsLayer, Default, Clone, Copy)]
pub(crate) enum CollisionLayer {
    /// Terrain, doodads and GameObjects, which both audiences collide with.
    #[default]
    Default,
    /// A WMO's walking collider, every face but DETAIL (`0x04`); only the body sees it.
    Walk,
    /// A WMO's camera/LOS collider, every face but NOCAMCOLLIDE (`0x02`); only the camera sees it.
    Camera,
    /// Liquid surfaces, every MCLQ layer's and WMO pool's wet cells, hit only by a query that asks.
    /// `cameraWaterCollision` ORs `0xf0000` into the trace mask the camera solver hands its three
    /// collision queries (`0x50e5ec`), and `0x69cc13` reads that nibble to gate the chunk's four
    /// MCLQ slots (`0x10000` river/lake, `0x20000` ocean, `0x40000` magma, `0x80000` slime).
    Liquid,
}

/// The camera probe's radius (yd) against the solid world. Deviation: the reference's camera
/// trace is a bare ray (`0x672170`); the sphere keeps the near plane out of a wall. Nothing may
/// hardcode it: a fixture sweeping another radius tests another client.
pub const CAMERA_PROBE_RADIUS: f32 = 0.3;

/// `CollisionLayers` for a WMO's walking collider, which the camera query never hits.
pub(crate) fn walk_layers() -> CollisionLayers {
    CollisionLayers::new(CollisionLayer::Walk, LayerMask::ALL)
}

/// `CollisionLayers` for a WMO's camera/LOS collider, which the body query never hits.
pub(crate) fn camera_layers() -> CollisionLayers {
    CollisionLayers::new(CollisionLayer::Camera, LayerMask::ALL)
}

/// `CollisionLayers` for a liquid surface, inert to every query that does not ask for it: a
/// swimmer is never stopped by the water they are in.
pub(crate) fn liquid_layers() -> CollisionLayers {
    CollisionLayers::new(CollisionLayer::Liquid, LayerMask::ALL)
}

/// How many times the world's collider set has changed: bumped when [`crate::terrain_stream`]
/// attaches a batch of colliders and when any collider is removed. A cached collision answer (the
/// creature ground clamp's) holds only at the stamp it was computed at.
#[derive(Resource, Default)]
pub struct ColliderEpoch(u64);

impl ColliderEpoch {
    /// The current stamp, recorded beside a cached collision answer.
    pub fn get(&self) -> u64 {
        self.0
    }

    /// Records a change. Any lane that inserts a collider outside the streamer's attach queue (the
    /// GameObject hull lane) must call it.
    pub fn bump(&mut self) {
        self.0 = self.0.wrapping_add(1);
    }
}

/// Bumps [`ColliderEpoch`] on any collider removal, despawns included; the streamer's attach loop
/// bumps for its own attaches.
pub(crate) fn track_collider_removals(
    mut removed: RemovedComponents<Collider>,
    mut epoch: ResMut<ColliderEpoch>,
) {
    if removed.read().count() > 0 {
        epoch.bump();
    }
}

/// Marks a static trimesh that receives ground decals (the selection ring). The reference collects
/// only terrain and WMO group faces (`0x6ad2c0` calls only `0x6ad330`/`0x6ad4e0`; draw flags
/// `0x200122`, liquid off), so the ring draws under a barrel, not onto it, and filters WMO faces
/// by MOPY class mask `0x88` alone, so it drapes down step faces. The mark rides the WMO walk
/// collider (every face but DETAIL `0x04`); a `0x88` face set of its own is not built.
#[derive(Component)]
pub struct GroundDecalSurface;

/// Marks a static collider that occludes the mouse pick: the reference traces the world too and
/// drops the object hit when the world hit is strictly nearer (`0x480df0` at `0x480eb4`,
/// `CWorld::Intersect` `0x672170`, mask `0x1000114`), so a unit behind a wall is not hoverable.
/// Occluders are terrain (DDA `0x69c920`), WMO group faces outside MOPY reject mask `0x84` (here
/// the walk collider, which rejects only `0x04`: a `0x84` face set is not built) and static
/// default-set doodad hulls (`0x69cdb0`). Liquid (mask bits 16-19 off) is not, nor are unit and
/// server-spawned GameObject hulls, transports included: those are the object trace.
#[derive(Component)]
pub struct PickOccluder;

/// Colliders the local mover's trace skips this frame (the game's half of the reference's
/// per-trace mask), applied by [`WorldCollision::cast_mover`]; empty on an ordinary frame.
#[derive(Resource, Default)]
pub struct MoverTraceExclusions(pub bevy::ecs::entity::EntityHashSet);

/// Traces a body through the world one-sided: the reference drops a face approached from its back
/// before any distance test, so the gate runs where candidates are enumerated ([`one_sided`]).
#[derive(bevy::ecs::system::SystemParam)]
pub struct WorldCollision<'w, 's> {
    ms: MoveAndSlide<'w, 's>,
    exclusions: bevy::ecs::system::Res<'w, MoverTraceExclusions>,
}

impl WorldCollision<'_, '_> {
    /// What a walking body collides with: the default layer and the WMO walking colliders. Public
    /// for the lanes that hand a filter elsewhere (the mouse pick's occlusion trace, the mount-tilt
    /// probe); the casts below apply it themselves.
    pub fn body_filter() -> SpatialQueryFilter {
        SpatialQueryFilter::from_mask(LayerMask(
            CollisionLayer::Default.to_bits() | CollisionLayer::Walk.to_bits(),
        ))
    }

    /// What the camera collides with: the default layer and the WMO camera/LOS colliders, so it
    /// hits DETAIL faces the body passes and passes NOCAMCOLLIDE faces the body stands on. The
    /// solid world only: the waterline rides [`waterline_ray`](Self::waterline_ray).
    pub(crate) fn camera_filter() -> SpatialQueryFilter {
        SpatialQueryFilter::from_mask(LayerMask(
            CollisionLayer::Default.to_bits() | CollisionLayer::Camera.to_bits(),
        ))
    }

    /// The waterline leg of the camera trace, a ray: `cameraWaterCollision` ORs `0xf0000` into the
    /// solver's trace mask (`0x50e5ec`), and that trace is a segment with no radius (`0x672170`,
    /// Möller–Trumbore at `0x7c2c40`). The swim corridor's `surface + 2/9` yd clearance is sized
    /// for that ray: a [`CAMERA_PROBE_RADIUS`] sphere on the corridor floor starts 78 mm through
    /// the plane. Two-sided, like the solid sweep.
    fn waterline_ray(&self, from: Vec3, movement: Vec3) -> Option<f32> {
        let Ok(dir) = Dir3::new(movement) else {
            return None;
        };
        self.ms
            .spatial_query
            .cast_ray(
                from,
                dir,
                movement.length(),
                true,
                &SpatialQueryFilter::from_mask(LayerMask(CollisionLayer::Liquid.to_bits())),
            )
            .map(|h| h.distance)
    }

    /// Sweep `shape` along `movement` against the body's world.
    pub fn cast_body(
        &self,
        shape: &Collider,
        from: Vec3,
        movement: Vec3,
        skin_width: f32,
    ) -> Option<MoveHitData> {
        self.cast_body_with(shape, from, movement, skin_width, &Self::body_filter())
    }

    /// [`cast_body`](Self::cast_body) against a caller-supplied filter: the mover's own trace mask,
    /// which the shared [`body_filter`](Self::body_filter) must not carry.
    fn cast_body_with(
        &self,
        shape: &Collider,
        from: Vec3,
        movement: Vec3,
        skin_width: f32,
        filter: &SpatialQueryFilter,
    ) -> Option<MoveHitData> {
        one_sided::cast_move(&self.ms, shape, from, movement, skin_width, filter)
    }

    /// Sweeps the camera boom: how far along `movement` the arm may reach, `None` when clear.
    /// Two-sided, since a camera has no facing contract. The solid world takes a
    /// [`CAMERA_PROBE_RADIUS`] sphere; with `liquid` (`cameraWaterCollision`) set, the waterline
    /// takes a ray ([`waterline_ray`](Self::waterline_ray)). The nearer hit wins.
    pub fn cast_camera(&self, from: Vec3, movement: Vec3, liquid: bool) -> Option<f32> {
        let solid = self
            .ms
            .cast_move(
                &Collider::sphere(CAMERA_PROBE_RADIUS),
                from,
                Quat::IDENTITY,
                movement,
                0.0,
                &Self::camera_filter(),
            )
            .map(|h| h.distance);
        let water = liquid.then(|| self.waterline_ray(from, movement)).flatten();
        match (solid, water) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// A one-sided ray against the body's world.
    pub fn ray_body(&self, origin: Vec3, dir: Dir3, max_distance: f32) -> Option<RayHitData> {
        one_sided::cast_ray(&self.ms, origin, dir, max_distance, &Self::body_filter())
    }

    /// Move `shape` by `velocity` for `delta_time`, sliding along what it hits.
    pub fn slide_body(
        &self,
        shape: &Collider,
        shape_position: Vec3,
        velocity: Vec3,
        delta_time: std::time::Duration,
        config: &MoveAndSlideConfig,
        on_hit: impl FnMut(MoveAndSlideHitData) -> MoveAndSlideHitResponse,
    ) -> MoveAndSlideOutput {
        self.slide_body_with(
            shape,
            shape_position,
            velocity,
            delta_time,
            config,
            &Self::body_filter(),
            on_hit,
        )
    }

    /// [`slide_body`](Self::slide_body) against a caller-supplied filter.
    fn slide_body_with(
        &self,
        shape: &Collider,
        shape_position: Vec3,
        velocity: Vec3,
        delta_time: std::time::Duration,
        config: &MoveAndSlideConfig,
        filter: &SpatialQueryFilter,
        on_hit: impl FnMut(MoveAndSlideHitData) -> MoveAndSlideHitResponse,
    ) -> MoveAndSlideOutput {
        one_sided::move_and_slide(
            &self.ms,
            shape,
            shape_position,
            velocity,
            delta_time,
            config,
            filter,
            on_hit,
        )
    }

    /// [`cast_body`](Self::cast_body) with this frame's [`MoverTraceExclusions`]. In the reference
    /// the mover's trace mask gains `0x8000` when it drives a player in ghost form (`0x631658`),
    /// and the GameObject candidacy virtual `0x5f85f0` then drops every DOOR (type 0) from the
    /// gather: a ghost walks through closed doors. Only the mover's own sweeps take it; the
    /// shared [`body_filter`](Self::body_filter) keeps its doors.
    pub fn cast_mover(
        &self,
        shape: &Collider,
        from: Vec3,
        movement: Vec3,
        skin_width: f32,
    ) -> Option<MoveHitData> {
        self.cast_body_with(shape, from, movement, skin_width, &self.mover_filter())
    }

    /// [`slide_body`](Self::slide_body) with this frame's [`MoverTraceExclusions`].
    pub fn slide_mover(
        &self,
        shape: &Collider,
        shape_position: Vec3,
        velocity: Vec3,
        delta_time: std::time::Duration,
        config: &MoveAndSlideConfig,
        on_hit: impl FnMut(MoveAndSlideHitData) -> MoveAndSlideHitResponse,
    ) -> MoveAndSlideOutput {
        self.slide_body_with(
            shape,
            shape_position,
            velocity,
            delta_time,
            config,
            &self.mover_filter(),
            on_hit,
        )
    }

    /// [`body_filter`](Self::body_filter) minus this frame's exclusions; no allocation when empty.
    fn mover_filter(&self) -> SpatialQueryFilter {
        Self::body_filter().with_excluded_entities(self.exclusions.0.iter().copied())
    }

    /// Every front-facing triangle in a box around `at`: the step probe's face gather.
    pub fn faces_near_body(&self, at: Vec3, half: Vec3, limit: usize) -> Vec<one_sided::FaceProbe> {
        one_sided::faces_near(&self.ms, at, half, &Self::body_filter(), limit)
    }
}

/// `cameraWaterCollision` is a trace mask (`0x50e5ec`): the waterline stops the camera boom only
/// with it set, and never the walking body.
#[cfg(test)]
mod liquid_trace_mask {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::*;

    /// A headless world holding one 10×10 horizontal surface on [`CollisionLayer::Liquid`].
    fn world_with_waterline() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
            PhysicsPlugins::new(bevy::app::PostUpdate),
        ));
        app.init_asset::<Mesh>();
        app.init_resource::<MoverTraceExclusions>();
        app.world_mut().spawn((
            RigidBody::Static,
            Collider::trimesh(
                vec![
                    Vec3::new(-5.0, 0.0, -5.0),
                    Vec3::new(5.0, 0.0, -5.0),
                    Vec3::new(5.0, 0.0, 5.0),
                    Vec3::new(-5.0, 0.0, 5.0),
                ],
                vec![[0u32, 2, 1], [0, 3, 2]],
            ),
            liquid_layers(),
            Transform::default(),
        ));
        app.finish();
        app.cleanup();
        app.update();
        app
    }

    /// Drop the camera boom from above the surface straight onto it.
    fn descend_camera(app: &mut App, liquid: bool) -> Option<f32> {
        app.world_mut()
            .run_system_once(move |c: WorldCollision| {
                c.cast_camera(Vec3::new(0.0, 3.0, 0.0), Vec3::new(0.0, -6.0, 0.0), liquid)
            })
            .expect("system runs")
    }

    /// The walking body on the same drop.
    fn descend_body(app: &mut App) -> Option<f32> {
        app.world_mut()
            .run_system_once(move |c: WorldCollision| {
                c.cast_body(
                    &Collider::sphere(CAMERA_PROBE_RADIUS),
                    Vec3::new(0.0, 3.0, 0.0),
                    Vec3::new(0.0, -6.0, 0.0),
                    0.0,
                )
                .map(|h| h.distance)
            })
            .expect("system runs")
    }

    /// A waterline at `y = 0` in real MCLQ cells (a chunk's 33.333 yd over 8, 4.167 yd): a sphere
    /// grazing one huge triangle is numerically unstable where one grazing a 4 yd cell is not.
    fn flooded_world() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
            PhysicsPlugins::new(bevy::app::PostUpdate),
        ));
        app.init_asset::<Mesh>();
        app.init_resource::<MoverTraceExclusions>();
        const CELL: f32 = 33.333_332 / 8.0;
        const N: usize = 32;
        let half = CELL * N as f32 * 0.5;
        let mut verts = Vec::with_capacity((N + 1) * (N + 1));
        for r in 0..=N {
            for c in 0..=N {
                verts.push(Vec3::new(
                    c as f32 * CELL - half,
                    0.0,
                    r as f32 * CELL - half,
                ));
            }
        }
        let mut tris = Vec::with_capacity(N * N * 2);
        for r in 0..N {
            for c in 0..N {
                let i = (r * (N + 1) + c) as u32;
                let stride = (N + 1) as u32;
                tris.push([i, i + stride, i + 1]);
                tris.push([i + 1, i + stride, i + stride + 1]);
            }
        }
        app.world_mut().spawn((
            RigidBody::Static,
            Collider::trimesh(verts, tris),
            liquid_layers(),
            Transform::default(),
        ));
        app.finish();
        app.cleanup();
        app.update();
        app
    }

    /// A surface-swimming human male: feet `0.75·h` under the plane, boom rooted at the capsule's
    /// top hemisphere centre, 15 yd of zoom.
    const SURFACE_DEPTH: f32 = 0.75 * 2.031;
    const HEAD_OVER_FEET: f32 = 2.027_777_7 - 1.0 / 3.0;
    const ZOOM: f32 = 15.0;
    /// The bare swim preset `cam+0x124` as framing pivot: an orbit centre 11 mm under the plane.
    const UNCORRECTED_PIVOT: f32 = 1.512_012;

    /// The settled arm length for a swimmer with feet at `feet_y`, framing `geo`, at `pitch` (+up).
    fn open_arm(app: &mut App, feet_y: f32, geo: (f32, f32), pitch: f32) -> f32 {
        let (head, boom, len) = boom_from(feet_y, geo, pitch);
        app.world_mut()
            .run_system_once(move |c: WorldCollision| {
                c.cast_camera(head, boom, true).unwrap_or(len)
            })
            .expect("system runs")
    }

    /// The positive control: the same arm as one sphere sweep with the liquid layer on its mask.
    fn open_arm_swept(app: &mut App, feet_y: f32, geo: (f32, f32), pitch: f32) -> f32 {
        let (head, boom, len) = boom_from(feet_y, geo, pitch);
        app.world_mut()
            .run_system_once(move |ms: MoveAndSlide| {
                ms.cast_move(
                    &Collider::sphere(CAMERA_PROBE_RADIUS),
                    head,
                    Quat::IDENTITY,
                    boom,
                    0.0,
                    &SpatialQueryFilter::from_mask(LayerMask(
                        CollisionLayer::Default.to_bits()
                            | CollisionLayer::Camera.to_bits()
                            | CollisionLayer::Liquid.to_bits(),
                    )),
                )
                .map_or(len, |h| h.distance)
            })
            .expect("system runs")
    }

    /// `(sweep origin, boom vector, its length)` for a swimmer at `feet_y` framing at `geo`.
    fn boom_from(feet_y: f32, geo: (f32, f32), pitch: f32) -> (Vec3, Vec3, f32) {
        let feet = Vec3::new(0.0, feet_y, 0.0);
        let head = feet + Vec3::Y * geo.1;
        let pivot = feet + Vec3::Y * geo.0;
        let fwd = Quat::from_euler(EulerRot::YXZ, 0.0, pitch, 0.0) * Vec3::NEG_Z;
        let boom = (pivot - fwd * ZOOM) - head;
        (head, boom, boom.length())
    }

    /// The uncorrected geometry: pivot at the bare swim preset, boom at the head, both on the body.
    fn uncorrected(_feet_y: f32) -> (f32, f32) {
        (UNCORRECTED_PIVOT, HEAD_OVER_FEET)
    }

    /// The corridor's geometry: pivot and sweep origin both at `surface + 2/9`, on the water plane.
    fn corrected(feet_y: f32) -> (f32, f32) {
        let floor = (0.0 - feet_y) + 2.0 / 9.0;
        (floor, HEAD_OVER_FEET.max(floor))
    }

    /// The worst one-step change in arm length as the swimmer's depth walks over `span`.
    fn worst_over_depth(
        app: &mut App,
        arm: impl Fn(&mut App, f32, (f32, f32), f32) -> f32,
        geo: impl Fn(f32) -> (f32, f32),
        pitch: f32,
        span: f32,
        n: usize,
    ) -> f32 {
        let mut worst: f32 = 0.0;
        let mut prev: Option<f32> = None;
        for i in 0..=n {
            let feet_y = -SURFACE_DEPTH - span * 0.5 + span * i as f32 / n as f32;
            let d = arm(app, feet_y, geo(feet_y), pitch);
            if let Some(q) = prev {
                worst = worst.max((d - q).abs());
            }
            prev = Some(d);
        }
        worst
    }

    /// A surface swimmer's depth settles by fractions of a millimetre with the mouse still. With
    /// the pivot 11 mm under the plane the boom's far end straddles it and the arm swings by yards;
    /// the corridor defines pivot and origin from the surface, so depth is no input at all.
    #[test]
    fn a_swimmers_settle_cannot_move_the_camera_once_the_corridor_holds_it() {
        let mut app = flooded_world();
        // The control at its worst pitch: slightly down, which swings the seat up across the plane.
        let control = worst_over_depth(
            &mut app,
            open_arm_swept,
            uncorrected,
            (-2.0f32).to_radians(),
            0.30,
            600,
        );
        assert!(
            control > 5.0,
            "the positive control must reproduce the regression — half a millimetre of settle \
             should swing the uncorrected camera by yards, got {control}"
        );
        // The corridor, across a swimmer's whole pitch range.
        for pitch_deg in [-25.0f32, -10.0, -2.0, -0.5, 0.0, 0.5, 2.0, 10.0, 25.0] {
            let step = worst_over_depth(
                &mut app,
                open_arm,
                corrected,
                pitch_deg.to_radians(),
                0.30,
                600,
            );
            assert!(
                step < 0.1,
                "at {pitch_deg} deg the settle moved the arm {step} yd — the corridor's whole \
                 claim is that depth is no longer an input (the control, for scale, was {control})"
            );
        }
    }

    /// A level boom behind a surface swimmer runs parallel to the water, 2/9 yd above it: a
    /// `0.3` yd sphere there starts 78 mm through the plane and pins at zero, a ray stays open.
    #[test]
    fn a_level_boom_behind_a_surface_swimmer_is_not_pinned_to_the_water() {
        let mut app = flooded_world();
        let feet_y = -SURFACE_DEPTH;
        let geo = corrected(feet_y);

        // The control, level: the sphere sweep must reproduce the pin.
        let control = open_arm_swept(&mut app, feet_y, geo, 0.0);
        assert!(
            control < 0.01,
            "the positive control must reproduce the regression — a level boom swept as a sphere \
             starts inside the plane and comes back pinned at zero, got {control}"
        );

        // The shipping query, across a surface swimmer's pitch band.
        for deg in [-25.0f32, -10.0, -2.0, -0.5, 0.0, 0.25, 0.5] {
            let open = open_arm(&mut app, feet_y, geo, deg.to_radians());
            assert!(
                open > ZOOM - 0.01,
                "at {deg} deg the arm came back clipped at {open} — the water is 2/9 yd below a \
                 boom that never descends to it"
            );
        }

        // A boom descending at 20 deg sheds the 2/9 yd clearance 0.65 yd along it (sin 20 deg).
        let aimed = open_arm(&mut app, feet_y, geo, 20.0f32.to_radians());
        assert!(
            (aimed - 0.65).abs() < 0.05,
            "aimed 20 deg up the boom must still stop ON the surface, got {aimed}"
        );
    }

    #[test]
    fn the_waterline_stops_the_camera_only_when_the_cvar_asks_for_it() {
        let mut app = world_with_waterline();
        let on = descend_camera(&mut app, true);
        assert!(
            on.is_some_and(|d| (d - 3.0).abs() < 0.01),
            "with cameraWaterCollision the boom stops at the surface, got {on:?}"
        );
        assert!(
            on.is_some_and(|d| d > 2.95),
            "and it stops ON the plane, not a probe radius short of it — the water leg is a ray, \
             which is what makes the corridor's 2/9 yd of clearance a clearance"
        );
        assert_eq!(
            descend_camera(&mut app, false),
            None,
            "with the CVar off the camera passes through, exactly as it did before this existed"
        );
        assert_eq!(
            descend_body(&mut app),
            None,
            "and the BODY passes through either way — a swimmer is not stopped by their own water"
        );
    }
}

/// The contact pipeline `world_plugins` disables: the stock broad phase pairs a resting kinematic
/// trimesh with the static world for a manifold every tick, and dropping it keeps the shape casts.
#[cfg(test)]
mod contact_pipeline {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::*;

    /// A static 10×10 floor and a kinematic 4×4 slab on it: a transport resting on terrain.
    fn overlapping_world(broad_phase: bool) -> App {
        let mut app = App::new();
        let physics = PhysicsPlugins::new(bevy::app::PostUpdate);
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
        ));
        if broad_phase {
            app.add_plugins(physics);
        } else {
            app.add_plugins(physics.build().disable::<BvhBroadPhasePlugin>());
        }
        app.init_asset::<Mesh>();
        // `update()` never runs plugin `finish()`, where avian seats its diagnostics resources.
        app.finish();
        app.cleanup();

        let quad = |half: f32, y: f32| {
            (
                vec![
                    Vec3::new(-half, y, -half),
                    Vec3::new(half, y, -half),
                    Vec3::new(half, y, half),
                    Vec3::new(-half, y, half),
                ],
                vec![[0u32, 2, 1], [0, 3, 2]],
            )
        };
        let (fv, ft) = quad(5.0, 0.0);
        app.world_mut().spawn((
            RigidBody::Static,
            Collider::trimesh(fv, ft),
            Transform::default(),
        ));
        // Coplanar and overlapping, so the AABBs intersect.
        let (sv, st) = quad(2.0, 0.0);
        app.world_mut().spawn((
            RigidBody::Kinematic,
            Collider::trimesh(sv, st),
            Transform::default(),
        ));
        // Two updates: the first seats `Position`/`Rotation` and the collider trees, and physics
        // first steps on the second.
        app.update();
        app.update();
        app
    }

    fn active_pairs(app: &App) -> usize {
        app.world().resource::<ContactGraph>().active_pairs().len()
    }

    #[test]
    fn the_stock_plugin_set_generates_contact_pairs_we_never_read() {
        // If this reads 0, avian changed and the broad-phase disable may no longer save anything.
        assert!(
            active_pairs(&overlapping_world(true)) > 0,
            "expected the stock broad phase to pair the kinematic slab with the static floor"
        );
    }

    #[test]
    fn dropping_the_broad_phase_removes_the_pairs_but_not_the_shape_casts() {
        let mut app = overlapping_world(false);
        assert_eq!(
            active_pairs(&app),
            0,
            "no broad phase means no contact pairs, so nothing to build manifolds for"
        );
        // The collider BVH is `ColliderTreePlugin`'s, not the broad phase's, so casts still work.
        let hit = app
            .world_mut()
            .run_system_once(|spatial: SpatialQuery| {
                spatial.cast_ray(
                    Vec3::new(0.0, 5.0, 0.0),
                    Dir3::NEG_Y,
                    10.0,
                    true,
                    &SpatialQueryFilter::default(),
                )
            })
            .expect("run_system_once");
        assert!(
            hit.is_some(),
            "the shape-cast lane must survive the broad phase being gone"
        );
    }
}
