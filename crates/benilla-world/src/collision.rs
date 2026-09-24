//! Physics **collision layers** — the two collision *audiences* the world geometry is queried by.
//!
//! The player body and the third-person camera collide against *different* sets of WMO faces (see
//! `benilla_formats`'s MOPY mask doc: the walking leaf `0x6bca50` and the camera/LOS leaf
//! `0x6bc700`): the **walking** gather drops DETAIL faces (`0x04`), the
//! **camera/LOS** gather instead drops NOCAMCOLLIDE faces (`0x02`) — so the camera collides with
//! visible decals/overhangs (forge pipes, low beams) the player walks *under*, and passes through
//! NOCAMCOLLIDE faces the player still stands on. avian can't filter a single trimesh per-face, so
//! each WMO bakes two colliders, separated by these layers; the player and camera queries pick their
//! audience by mask. Everything else — terrain, doodads, GameObjects — sits on the default layer and is
//! collided by **both** (so it needs no explicit layer).

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

/// The collision audiences. avian reserves bit 0 for [`CollisionLayer::Default`], which every collider
/// without an explicit `CollisionLayers` belongs to — so terrain/doodad/GameObject colliders are seen by
/// both the player and camera queries automatically. Only the two per-WMO meshes carry an explicit layer.
#[derive(PhysicsLayer, Default, Clone, Copy)]
pub(crate) enum CollisionLayer {
    /// Geometry both audiences collide with: terrain, doodads, GameObjects (the default layer).
    #[default]
    Default,
    /// WMO faces only the **player body** collides with — the walking gather (skip DETAIL `0x04`).
    Walk,
    /// WMO faces only the **camera** collides with — the camera/LOS gather (skip NOCAMCOLLIDE `0x02`).
    Camera,
    /// **Liquid surfaces** — the wet-cell lattice of every MCLQ layer and WMO pool, on a layer of
    /// their own so that *no* query sees them unless it asks.
    ///
    /// This is `cameraWaterCollision`'s primary consumer and the shape the reference gives it: the
    /// CVar does not re-lane any geometry, it adds `0xf0000` to the **trace mask** the solver hands
    /// its three collision queries (`0x50e5ec`; the nibble is read at `0x69cc13` and gates a
    /// per-layer intersection over the chunk's four MCLQ slots — `0x10000` river/lake, `0x20000`
    /// ocean, `0x40000` magma, `0x80000` slime). A per-trace mask is exactly a `SpatialQueryFilter`,
    /// which is why this is a layer and not a component the camera looks for.
    ///
    /// An earlier reading judged this capability unreachable, and the verdict stood for months —
    /// wrongly: a capability requested through an argument flag is invisible to a plain call-site
    /// search. Decision 2149 rested benilla on that verdict and built the wrong
    /// half of the feature; 2165 took it out.
    Liquid,
}

/// **The camera-collision probe radius (yd)** — the sphere [`WorldCollision::cast_camera`] sweeps
/// along the boom against the SOLID world.
///
/// It is the margin kept between the camera and the surface it stops at, so the near plane does
/// not poke through the wall; smaller than the player capsule, so the camera threads gaps the body
/// cannot. **benilla's own construction** — the reference's camera trace is a bare ray
/// (`0x672170`) with no radius anywhere on it.
///
/// It lives *here*, in the crate that owns the trace, rather than beside the camera that spawns
/// the probe, because a fixture that sweeps a different radius than the client ships is an
/// instrument reading itself: decision 2179's gate swept `0.15` against a client shipping `0.3`
/// and certified a camera that was being slammed onto the character on every level surface swim
/// (2185). Nothing may hardcode this number.
pub const CAMERA_PROBE_RADIUS: f32 = 0.3;

/// `CollisionLayers` for a WMO's **walking** collider — member of [`CollisionLayer::Walk`] only, so the
/// camera query (which omits `Walk`) never hits it.
pub(crate) fn walk_layers() -> CollisionLayers {
    CollisionLayers::new(CollisionLayer::Walk, LayerMask::ALL)
}

/// `CollisionLayers` for a WMO's **camera/LOS** collider — member of [`CollisionLayer::Camera`] only, so
/// the player movement query (which omits `Camera`) never hits it.
pub(crate) fn camera_layers() -> CollisionLayers {
    CollisionLayers::new(CollisionLayer::Camera, LayerMask::ALL)
}

/// `CollisionLayers` for a **liquid surface** — member of [`CollisionLayer::Liquid`] only.
///
/// Every existing filter omits that bit, so these colliders are inert to the body, the camera, the
/// mouse pick and the particle snap alike until a query asks for them by name. A swimmer must not
/// be stopped by the water they are in, and nothing but the camera sweep under
/// `cameraWaterCollision` has any business hitting a waterline.
pub(crate) fn liquid_layers() -> CollisionLayers {
    CollisionLayers::new(CollisionLayer::Liquid, LayerMask::ALL)
}

/// **How many times the world's collider set has changed** — bumped when [`crate::terrain_stream`]
/// attaches a batch of built colliders, and when any collider is removed (a tile or placement
/// streaming out, an entity despawning).
///
/// It exists because a cached collision *answer* is only as good as the world it was computed
/// against. The one consumer today is the creature ground clamp's cast gate (decision 1357), which
/// holds a hit while the unit's own inputs are unchanged: the unit's position and the colliders
/// under it. It could see the first half change and not the second, so a unit that took its answer
/// from a half-arrived world — the terrain under a building, before the building's own floor
/// collider had attached — held that answer forever and stood under the floor for the rest of the
/// session. A stamp on the collider set closes that: an answer computed at one stamp is re-asked at
/// the next.
///
/// A `u64` counter, not a dirty flag: the gate compares its own recorded stamp, so it never has to
/// coordinate with other readers about who clears it.
#[derive(Resource, Default)]
pub struct ColliderEpoch(u64);

impl ColliderEpoch {
    /// The current stamp — recorded alongside a cached collision answer, compared to decide whether
    /// that answer still describes this world.
    pub fn get(&self) -> u64 {
        self.0
    }

    /// **The collider set changed** — call this from any lane that inserts a collider outside the
    /// streamer's own attach queue (the GameObject hull lane is the one such lane today, decision
    /// 0761). A lane that forgets leaves a cached answer dated against a world that no longer
    /// exists; the cost of an over-eager bump is one re-cast.
    pub fn bump(&mut self) {
        self.0 = self.0.wrapping_add(1);
    }
}

/// Bump [`ColliderEpoch`] for the *removal* half (the attach half is published by the streamer's
/// own attach loop, which already counts what it did). Despawning an entity emits removal events
/// for its components, so this covers tile/placement stream-out and object destroys alike; reading
/// the queue is O(removals), which is zero on almost every frame.
pub(crate) fn track_collider_removals(
    mut removed: RemovedComponents<Collider>,
    mut epoch: ResMut<ColliderEpoch>,
) {
    if removed.read().count() > 0 {
        epoch.bump();
    }
}

/// Marks a static trimesh collider whose triangles **receive ground decals** (the selection ring).
/// The reference's decal collector is byte-verified (the box-query dispatch `0x6ad2c0` calls only
/// its WMO and terrain collectors, `0x6ad330`/`0x6ad4e0`):
/// its box query gathers **terrain triangles + WMO group faces** — M2 doodads/GameObjects/units are
/// *never* collected (no collector exists for them; the ring draws under barrels, not onto them) —
/// with the decal draw sites passing flags `0x200122` (terrain on, WMO on, liquid off). The WMO side
/// filters faces only by a MOPY **material-class mask `0x88`** (no walkability/up-facing test — which
/// is why the reference ring drapes down vertical step faces). We mark the WMO **walk** collider as
/// the interim WMO face set — all faces minus DETAIL (`0x04`) — which differs from the reference's
/// minus-`0x88`-classes set only on those flagged faces (exact parity would need a third per-WMO face
/// bake; deferred until the MOPY `0x88` class semantics are RE'd).
#[derive(Component)]
pub struct GroundDecalSurface;

/// Marks a static collider that **occludes the mouse pick** — the reference's scene trace also
/// traces the world and discards the object hit iff the world hit is *strictly nearer*
/// (`0x480df0` @ `0x480eb4`,
/// `CWorld::Intersect 0x672170` mask `0x1000114`), so a unit/GameObject behind a wall is not
/// hoverable. The byte-decoded occluder set, and what carries the mark: **terrain** tiles (DDA
/// `0x69c920`), **WMO group faces** with MOPY reject-mask `0x84` — the **walk** bake (reject
/// `0x04`) is the nearest existing face set, differing only on `0x80`-flagged faces (exact parity
/// needs a third per-WMO bake, the same deferral as the decal path's `0x88`) — and **static
/// default-set map doodad hulls** (`0x69cdb0`; map doodads + WMO props here). NOT marked, per the
/// same decode: liquid (mask bits 16-19 are off on the pick path), server-spawned GameObject
/// hulls and units (they are the *object* trace — a chest must not occlude itself), and transport
/// hulls (the 0466 law: an NPC on deck stays hoverable through the railing).
#[derive(Component)]
pub struct PickOccluder;

/// **Colliders this frame's local mover must not see** — the game's half of the reference's
/// per-trace collision mask.
///
/// The engine cannot compute this: "a GameObject whose type is DOOR, while the player is a ghost"
/// names two gameplay concepts, and the engine names nothing of the game (this crate's other API
/// wall). So the game publishes the entity set and [`WorldCollision::cast_mover`] applies it.
///
/// Empty on every ordinary frame, which is what makes the living player's query provably the one it
/// always was.
#[derive(Resource, Default)]
pub struct MoverTraceExclusions(pub bevy::ecs::entity::EntityHashSet);

/// **Trace a body through the world** — the engine's movement-collision face.
///
/// Every one of the four one-sided casts took the same two leading arguments (avian's
/// `MoveAndSlide`, and a `SpatialQueryFilter` the caller built on the line before), and every
/// caller built the *same* filter. Both are the engine's business: which collision layers a body
/// sees is a property of how the world was baked (the two per-WMO face sets, `CollisionLayer`),
/// not a choice a mover gets to make.
///
/// **Deliberately not merged with `WorldPoint`** (1164 item 11): `ground_normal_under` here and
/// `terrain_height_under` there answer "what is the ground" from two different worlds — a physics
/// trimesh and an MCNK heightfield — and merging them would put avian's `MoveAndSlide` in the
/// signature of every zone-text and footstep reader.
///
/// The **one-sidedness** is the whole reason these are not avian's own calls: the reference
/// discards a face approached from its back before computing any distance, and that gate has to
/// run where candidates are enumerated (see [`one_sided`]'s header, decisions 0967/0968).
#[derive(bevy::ecs::system::SystemParam)]
pub struct WorldCollision<'w, 's> {
    ms: MoveAndSlide<'w, 's>,
    exclusions: bevy::ecs::system::Res<'w, MoverTraceExclusions>,
}

impl WorldCollision<'_, '_> {
    /// What a walking **body** collides with: the default layer (terrain, doodads, GameObjects)
    /// plus the walk-only WMO faces — and never the camera-only ones.
    ///
    /// Exposed for the two lanes that hand a filter to something other than these casts (the mouse
    /// pick's occlusion trace, the mount-tilt probe); everything else should use the methods
    /// below, which apply it themselves.
    pub fn body_filter() -> SpatialQueryFilter {
        SpatialQueryFilter::from_mask(LayerMask(
            CollisionLayer::Default.to_bits() | CollisionLayer::Walk.to_bits(),
        ))
    }

    /// …and what the third-person **camera** collides with: the other way round on those two WMO
    /// face layers (it takes NOCAMCOLLIDE faces the body walks through, and skips the DETAIL faces
    /// the body walks on).
    ///
    /// **The waterline is deliberately not in here** — it rides its own ray
    /// ([`waterline_ray`](Self::waterline_ray)), for the reason spelled out there. This mask is the
    /// SOLID world only, and is byte-for-byte the filter it always was.
    pub(crate) fn camera_filter() -> SpatialQueryFilter {
        SpatialQueryFilter::from_mask(LayerMask(
            CollisionLayer::Default.to_bits() | CollisionLayer::Camera.to_bits(),
        ))
    }

    /// **The waterline leg of the camera trace — a RAY, not the probe sphere** (decision 2185).
    ///
    /// `cameraWaterCollision` is a trace mask (`0x50e5ec` ORs `0xf0000` into the word the solver
    /// hands its three collision queries), and the query that word rides is a **line segment**:
    /// `0x672170` takes `(start, end, out, frac, flags)` and carries no radius at all, bottoming
    /// out in the Möller–Trumbore ray/triangle test at `0x7c2c40`. The reference's whole
    /// camera trace is that ray; benilla's is a [`CAMERA_PROBE_RADIUS`] sphere, which is **our own
    /// construction** — the margin that keeps the near plane out of a wall — and it stays, because
    /// walls are what it is for.
    ///
    /// It cannot stay for water, because the corridor's clearance is a reference constant sized
    /// for a ray. Arm A lifts the sweep origin to `surface + 2/9 = 0.2222` yd, and the probe's
    /// radius is `0.3`: centred on that floor the sphere hangs **78 mm through the water plane
    /// before the sweep has moved at all**, so a level camera behind a surface swimmer was returned
    /// a hit at distance zero — the camera slammed onto the character, every frame the boom ran
    /// flat. Tip it a couple of degrees down and the sweep escapes the plane and the hit vanishes;
    /// that flip, against a snap-in/ease-out arm, is the "snaps in for a second every couple of
    /// seconds" a surface swim was reported with.
    ///
    /// Two-sided, like the shape sweep beside it and for the same reason: a camera crossing the
    /// surface from underneath has no facing contract to honour, it just must not end up through
    /// the plane.
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

    /// [`cast_body`](Self::cast_body) against a **caller-supplied** filter.
    ///
    /// It exists for one caller and one reason: the reference's world trace carries a per-trace
    /// mask, and a GHOST's mask drops DOOR GameObjects from the gather — a ghost walks through
    /// closed doors (the GameObject collision-candidacy virtual `0x5f85f0`). That is a property of
    /// *this mover's trace*, not of the door, so it cannot be expressed by re-laning the collider:
    /// [`body_filter`](Self::body_filter) is shared with the particle snap, the precipitation
    /// probe, the mouse pick and the creature conform, none of which may lose a door because the
    /// local player happens to be dead.
    ///
    /// The living case passes [`body_filter`](Self::body_filter) itself, so it is the same query it
    /// always was — byte for byte, by construction rather than by inspection.
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

    /// Sweep the **camera boom** against the camera's world — the distance along `movement` the
    /// arm is free to reach, or `None` for an unobstructed boom.
    ///
    /// Deliberately avian's own two-sided cast rather than the one-sided law above: the facing
    /// gate exists because a *body* must not stand on a face wound away from it, and a camera has
    /// no such contract — it just must not end up inside geometry, from either side.
    ///
    /// **Two legs, two shapes, and that is the point** (decision 2185). The solid world is swept
    /// with the caller's probe sphere, which is benilla's own margin against a wall's near plane;
    /// the waterline is a bare ray, which is the reference's own geometry and the only shape that
    /// fits through the clearance the corridor budgets for it — see
    /// [`waterline_ray`](Self::waterline_ray). `liquid` is `cameraWaterCollision`, straight
    /// through: with it clear the water leg does not run and this is the query it always was.
    ///
    /// The nearer of the two wins, which is what one trace carrying both classes would have done.
    ///
    /// **The probe is this function's, not the caller's.** It used to be threaded in from a
    /// `CameraProbe` resource in `benilla-app`, along with a rotation that was always the identity
    /// (a sphere has no orientation) and a skin width that was always zero — three arguments of
    /// noise around one number that belongs to the trace. That number is [`CAMERA_PROBE_RADIUS`],
    /// and the whole of decision 2185 is that it must be read in the same place the clearance it
    /// has to fit through is (`crates/benilla-app/tests/world_api_wall.rs` is what made the point:
    /// exporting the constant widened the doorway, and owning the probe closes it instead).
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

    /// [`slide_body`](Self::slide_body) against a caller-supplied filter — the ghost's door
    /// exclusion, for the reason spelled out on [`cast_body_with`](Self::cast_body_with).
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

    /// Sweep the **local mover's own body** — [`cast_body`](Self::cast_body) with this frame's
    /// [`MoverTraceExclusions`] applied.
    ///
    /// The reference's world trace carries a per-trace mask, and the local mover's mask gains bit
    /// `0x8000` when the body it is driving is a player **in ghost form** (`0x631658`); the
    /// GameObject collision-candidacy virtual `0x5f85f0` reads that bit and drops every
    /// `GAMEOBJECT_TYPE_ID == 0` (DOOR) object from the gather. So a ghost walks through closed
    /// doors, and nothing else about its collision differs.
    ///
    /// **Why the exclusion rides the trace and not the collider.** It is tempting to drop the
    /// door's collider, or re-lane it — both are wrong, because the fact is about *whose trace this
    /// is*. [`body_filter`](Self::body_filter) is shared with the particle snap, the precipitation
    /// probe, the mouse pick, the taxi probe and the creature ground conform; a door that stopped
    /// existing for all of them because the local player happened to be dead would be a much larger
    /// claim than the binary makes. Only the mover's own sweeps take the exclusion.
    ///
    /// With the set empty — every living frame — this is [`cast_body`](Self::cast_body) exactly.
    pub fn cast_mover(
        &self,
        shape: &Collider,
        from: Vec3,
        movement: Vec3,
        skin_width: f32,
    ) -> Option<MoveHitData> {
        self.cast_body_with(shape, from, movement, skin_width, &self.mover_filter())
    }

    /// [`slide_body`](Self::slide_body) for the local mover — see [`cast_mover`](Self::cast_mover).
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

    /// [`body_filter`](Self::body_filter), plus whatever this frame's mover must not see.
    ///
    /// Allocation-free while the set is empty (`EntityHashSet::default()` allocates nothing), which
    /// is every frame the player is alive.
    fn mover_filter(&self) -> SpatialQueryFilter {
        Self::body_filter().with_excluded_entities(self.exclusions.0.iter().copied())
    }

    /// Every front-facing triangle in a box around `at` — the step probe's face gather.
    pub fn faces_near_body(&self, at: Vec3, half: Vec3, limit: usize) -> Vec<one_sided::FaceProbe> {
        one_sided::faces_near(&self.ms, at, half, &Self::body_filter(), limit)
    }
}

/// **`cameraWaterCollision` is a trace mask and nothing else.** The same waterline collider is a
/// wall to the camera boom with the CVar on, thin air with it off, and thin air to the walking body
/// either way — which is the whole of the reference's `0x50e5ec`, and the reason a swimmer is never
/// stopped by the water they are in.
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

    /// …and the walking body, which must pass through either way.
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

    /// A waterline at `y = 0`, tessellated at the **real** liquid cell size.
    ///
    /// Deliberately not one huge quad: an MCNK liquid layer is a 9×9 lattice over the chunk's
    /// 33.333 yd, so its cells are 4.167 yd and its triangles are that big and no bigger. A sphere
    /// swept at grazing incidence against a 120 yd triangle is numerically unstable in a way it is
    /// not against a 4 yd one, and a test that models the water as one quad measures the solver's
    /// conditioning rather than the client's behaviour — which is exactly the sort of
    /// unrepresentative fixture that lets a defect through.
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
        // 32×32 cells of 4.1667 yd — 133 yd square, 2048 triangles, the shape a streamed
        // MCLQ layer actually presents.
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
    /// What 2170 framed at — the bare swim preset `cam+0x124`, which puts the orbit centre 11 mm
    /// UNDER the plane.
    const UNCORRECTED_PIVOT: f32 = 1.512_012;

    /// The arm length the solver settles on for a swimmer whose feet are at `feet_y`, framing at
    /// `pivot_over_feet`, sweeping from `origin_over_feet`, looking at `pitch` radians (+up).
    fn open_arm(app: &mut App, feet_y: f32, geo: (f32, f32), pitch: f32) -> f32 {
        let (head, boom, len) = boom_from(feet_y, geo, pitch);
        app.world_mut()
            .run_system_once(move |c: WorldCollision| {
                c.cast_camera(head, boom, true).unwrap_or(len)
            })
            .expect("system runs")
    }

    /// The same arm through the query benilla shipped **before** decision 2185: one sphere sweep
    /// carrying the liquid layer on its own mask, exactly as `camera_filter(true)` built it.
    ///
    /// It stays as the positive control for both of the tests below, because it is the query in
    /// which the defect lives — and a gate whose control cannot fail is not a gate.
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

    /// 2170's geometry at a given depth: the framing pivot is the bare swim preset, so it rides the
    /// **body** — and the boom is rooted at the head, likewise.
    fn uncorrected(_feet_y: f32) -> (f32, f32) {
        (UNCORRECTED_PIVOT, HEAD_OVER_FEET)
    }

    /// The corridor's geometry at a given depth: arm A pins the pivot to `d + 2/9` and its floor
    /// lifts the sweep origin to the same place, so **both ride the water plane** rather than the
    /// body.
    fn corrected(feet_y: f32) -> (f32, f32) {
        let floor = (0.0 - feet_y) + 2.0 / 9.0;
        (floor, HEAD_OVER_FEET.max(floor))
    }

    /// Worst single-step change in arm length as the swimmer's depth is walked over `span`, at the
    /// given pitch.
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

    /// **The gate this feature ships behind, with a positive control that reproduces the bug.**
    ///
    /// The axis is **depth**, not pitch — which is the correction that made this test worth having.
    /// A first attempt swept the camera *pitch* and found all three geometries merely steep, no
    /// cliff anywhere; that was the wrong question, because the director's report was of a camera
    /// snapping *while swimming at the surface*, with the mouse still. What varies then is the
    /// swimmer's depth, by fractions of a millimetre, as the float resolver settles against its
    /// rest cap.
    ///
    /// **2170's geometry turns half a millimetre of that into ten yards of camera.** Its framing
    /// pivot is the bare swim preset, so the orbit centre rides the BODY — and sits 11 mm under the
    /// water plane. The boom's far end therefore straddles the plane, and whether the sweep hits at
    /// all flips with the settle. Snap in, ease out, snap in: exactly what was reported.
    ///
    /// **The corridor's geometry cannot do this, structurally.** Arm A pins the pivot to `d + 2/9`
    /// and its floor lifts the sweep origin to the same place — both defined FROM THE SURFACE, so
    /// the camera's clearance over the water is a constant 2/9 yd no matter what the body does
    /// underneath it. Depth stops being an input. That is the property worth having, and it is why
    /// this is a fix rather than a tuning: nothing here was made smaller, it was made independent.
    #[test]
    fn a_swimmers_settle_cannot_move_the_camera_once_the_corridor_holds_it() {
        let mut app = flooded_world();
        // The control, at the pitch where the old geometry is worst: looking slightly down, which
        // swings the seat up across the plane.
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
             should swing 2170's camera by yards, got {control}"
        );
        // …and the corridor, across the whole range a swimmer looks through.
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

    /// **The probe that did not fit through its own corridor** (decision 2185).
    ///
    /// The corridor lifts the camera's sweep origin to `surface + 2/9 = 0.2222` yd. That is a
    /// **reference** constant, and the reference's trace is a bare ray: `0x672170` takes a start,
    /// an end, an out-point and a `frac` and carries no radius anywhere, bottoming out in the
    /// Möller–Trumbore ray/triangle test at `0x7c2c40`. benilla's camera probe is a
    /// [`CAMERA_PROBE_RADIUS`] = `0.3` yd sphere — **our own** construction, the margin that keeps
    /// the near plane out of a wall. `0.3 > 0.2222`, so centred on the corridor floor it hangs 78
    /// mm through the water plane *before the sweep has moved at all*.
    ///
    /// A surface swimmer's boom runs from that origin to a seat at the same height — the corridor
    /// pins both to `surface + 2/9` — so a **level** camera sweeps exactly parallel to the water,
    /// and came back pinned at zero: the camera slammed onto the character. Tip it two degrees
    /// down and the sphere escapes the plane and the hit vanishes. That flip, against an arm that
    /// snaps in instantly and eases back out over about a second, is the "snaps in for a second
    /// every couple of seconds" a surface swim was reported with — and it is the same mechanism
    /// 2170 was reported for and 2179 could not reach, because the corridor it built is measured
    /// in a clearance this probe was always too fat for.
    #[test]
    fn a_level_boom_behind_a_surface_swimmer_is_not_pinned_to_the_water() {
        let mut app = flooded_world();
        let feet_y = -SURFACE_DEPTH;
        let geo = corrected(feet_y);

        // The control, which must reproduce the defect: the pre-2185 query, level.
        let control = open_arm_swept(&mut app, feet_y, geo, 0.0);
        assert!(
            control < 0.01,
            "the positive control must reproduce the regression — a level boom swept as a sphere \
             starts inside the plane and comes back pinned at zero, got {control}"
        );

        // …and the shipping query, across the band a surface swimmer actually looks through.
        for deg in [-25.0f32, -10.0, -2.0, -0.5, 0.0, 0.25, 0.5] {
            let open = open_arm(&mut app, feet_y, geo, deg.to_radians());
            assert!(
                open > ZOOM - 0.01,
                "at {deg} deg the arm came back clipped at {open} — the water is 2/9 yd below a \
                 boom that never descends to it"
            );
        }

        // …and the option still does its job when the camera really is aimed under the water:
        // 2/9 yd of clearance shed at sin(20 deg) crosses the plane 0.65 yd along the boom.
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
            "and it stops ON the plane, not a probe radius short of it — the water leg is a ray \
             (decision 2185), which is what makes the corridor's 2/9 yd of clearance a clearance"
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

/// The contact pipeline we deliberately do not run (decision 1232, disabled in `world_plugins`).
///
/// These two tests are a matched pair: the first shows the cost was real — a kinematic trimesh
/// resting in the static world generates contact pairs, and avian then computes a trimesh-vs-trimesh
/// manifold for each, every physics tick — and the second shows that dropping the broad phase
/// removes them *without* costing the shape-casts the character controller actually rides.
#[cfg(test)]
mod contact_pipeline {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::*;

    /// A static 10×10 floor plus a kinematic 4×4 slab overlapping it — the transport-on-terrain
    /// shape that was generating manifolds against 65k-triangle tiles in the real world.
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
        // Overlapping, not merely adjacent: same plane, so the AABBs intersect and the narrow
        // phase has real triangle work to do.
        let (sv, st) = quad(2.0, 0.0);
        app.world_mut().spawn((
            RigidBody::Kinematic,
            Collider::trimesh(sv, st),
            Transform::default(),
        ));
        // Two frames, not one: the first seats `Position`/`Rotation` and the collider trees, and
        // the physics schedule first steps on the second. A single-`update()` fixture (which is
        // what `one_sided`'s is) never runs a physics step at all.
        app.update();
        app.update();
        app
    }

    fn active_pairs(app: &App) -> usize {
        app.world().resource::<ContactGraph>().active_pairs().len()
    }

    #[test]
    fn the_stock_plugin_set_generates_contact_pairs_we_never_read() {
        // The bug's premise. If this ever reads 0, avian changed and 1232's reasoning needs a
        // re-read before the disable below can still be justified as a saving.
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
        // The half that must NOT regress: the collider BVH is `ColliderTreePlugin`'s, not the
        // broad phase's, so a downward cast still finds the floor the player walks on.
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
