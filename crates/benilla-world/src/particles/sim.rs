use avian3d::prelude::SpatialQuery;
use benilla_assets::coords::{bevy_to_wow, wow_to_bevy};
use bevy::camera::primitives::{Frustum, Sphere as CullSphere};
use bevy::camera::visibility::RenderLayers;
use bevy::camera::Projection;
use bevy::prelude::*;

use crate::view::WorldCamera;

use super::buffer::{
    EffectDrawSpec, EffectFog, EffectLightOverride, EffectLighting, EffectQuads, EffectVertex,
};
use super::quads::{expand_quads, CamBasis, DrawFrame};
use super::{
    accumulate_emission, emit_local, next_u32, rand01, rand_s11, ChildDraw, ChildEmitter,
    EmitterFade, OwnerLoss, Particle, ParticleEmitter, ParticleTuning, MAX_PARTICLES,
};

/// The emitter-constant inputs of one integrator step ([`integrate_particle`]).
struct StepEnv {
    dt: f32,
    gravity: f32,
    drag: f32,
    anchored: bool,
    kill_origin: Option<Vec3>,
    /// The frame's shared follow-delta vector, stored frame (zero when unauthored).
    follow: Vec3,
}

/// One step of the reference integrator (`0x7b2680`): age and kill; the follow-delta add, skipped
/// on a fresh particle (`0x7b2744`); `pos += dt·v` with gravity on the frame's up axis
/// (`pos.up −= ½·g·dt²`, `v.up −= g·dt`); drag `v −= min(dt·drag, 1)·v`. A kill-outbound emitter
/// (`rt 0x800`) kills once `dot(stepVelocity, pos − origin) > 0`, with the pre-gravity velocity
/// and the emitter origin in the stored frame. False is dead.
fn integrate_particle(p: &mut Particle, env: &StepEnv) -> bool {
    let (dt, g) = (env.dt, env.gravity);
    p.age += dt;
    // Its own birth-sampled lifetime: the reference feeds each spawn the current lifespan.
    if p.age >= p.life {
        return false;
    }
    // Follow-delta (file 0x4000, `0x7b2744`), skipped on the first integrate (particle+0xd).
    if p.fresh {
        p.fresh = false;
    } else {
        p.pos += env.follow;
    }
    // Model-particle tumble (`0x7b28e0`): a body-frame Rodrigues step, skipped under 1e-4.
    let theta = p.angvel.length();
    if theta > 1e-4 {
        p.quat = (p.quat * Quat::from_axis_angle(p.angvel / theta, theta * dt)).normalize();
    }
    let step_vel = p.vel;
    p.pos += p.vel * dt;
    if env.anchored {
        p.pos.y -= 0.5 * g * dt * dt;
        p.vel.y -= g * dt;
    } else {
        p.pos.z -= 0.5 * g * dt * dt;
        p.vel.z -= g * dt;
    }
    if env.drag != 0.0 {
        let f = (dt * env.drag).min(1.0);
        p.vel -= f * p.vel;
    }
    if let Some(origin) = env.kill_origin {
        if step_vel.dot(p.pos - origin) > 0.0 {
            return false;
        }
    }
    true
}

/// The follow-delta fraction (file flag `0x4000`, rt `0x40000`): the share of the emitter's
/// per-frame translation added to every live particle, keyed on `speed` = |Δ|/dt in yd/s; zero
/// when unauthored or degenerate. The ride-vs-trail baseline is the storage space, not this term,
/// which applies whatever `0x10` says (`0x7b5303` builds `rt+0x26c`, `0x7b2744` adds `rt+0x278`).
fn follow_fraction(def: &benilla_formats::ParticleEmitterDef, speed: f32) -> f32 {
    if !def.follow_emitter() {
        return 0.0;
    }
    def.follow_line().map_or(0.0, |(slope, intercept)| {
        (slope * speed + intercept).clamp(0.0, 1.0)
    })
}

/// The velocity-inherit trigger (`0x7b5230`, `0x7b53ce`..`0x7b54ca`): past 1/30 s of dt
/// (`0x81d82c`), hold `oneFrameΔ · ((1/30)/accum) · scale`, zero with nothing live, and reset.
fn inherit_trigger(accum: &mut f32, held: &mut Vec3, dt: f32, delta: Vec3, live: bool, scale: f32) {
    const INTERVAL: f32 = 1.0 / 30.0;
    *accum += dt;
    if *accum > INTERVAL {
        *held = if live {
            delta * (INTERVAL / *accum) * scale
        } else {
            Vec3::ZERO
        };
        *accum = 0.0;
    }
}

/// The child drive (`0x7b5b9f`): a child's `rate·dt` accumulation runs once per live parent
/// particle, births landing at that particle through the parent's rotation (the child's record
/// position never composes). Child flag 0x40 adds `(1 + S11·var)·v` of the parent particle's
/// velocity (`0x7b5b5e`). A child never emits on its own.
fn drive_child(
    child: &mut ChildEmitter,
    now: &benilla_formats::ParamsNow,
    parent: &[Particle],
    rate: f32,
    emitting: bool,
    scale: f32,
    dt: f32,
    anchored: bool,
    placement: &Transform,
) {
    let origin = Vec3::from(child.def.position);
    for p in parent {
        accumulate_emission(
            child.def.burst(),
            rate,
            emitting,
            scale,
            dt,
            &mut child.accumulator,
            &mut child.gate_prev,
        );
        while child.accumulator >= 1.0 && child.particles.len() < MAX_PARTICLES {
            child.accumulator -= 1.0;
            let (base, dir) = emit_local(&child.def, now, &mut child.rng);
            let local = base - origin;
            let speed = now.emission_speed * (1.0 + now.speed_variation * rand_s11(&mut child.rng));
            // In world mode the parent particle is in world: the fold is rotation and scale only.
            let fold = |v: Vec3| {
                if anchored {
                    placement.rotation * (placement.scale * wow_to_bevy(v.to_array()))
                } else {
                    v
                }
            };
            let mut vel = fold(dir * speed);
            if child.def.inherits_emitter_motion() {
                vel += (1.0 + now.speed_variation * rand_s11(&mut child.rng)) * p.vel;
            }
            let phase = next_u32(&mut child.rng);
            child.particles.push(Particle {
                pos: p.pos + fold(local),
                vel,
                age: birth_age(child.def.burst(), dt, &mut child.rng),
                life: now.lifespan,
                phase,
                fresh: true,
                quat: Quat::IDENTITY,
                angvel: Vec3::ZERO,
            });
        }
    }
}

/// Birth age `U01 · w`, the kernels' third argument (`0x7b88c4`, `0x7b88c7`): `w` is 0 for a burst
/// (`0x7b5600`) and the substep dt for a pour (`0x7b5779`, `0x7b57e2`), so a stream's same-frame
/// births de-synchronise. Deviation: `w` is the frame dt clamped to 0.1 s, because we do not
/// substep as the reference does at 0.1 s (`0x81d828`, `0x7b58e7`); they differ only on a hitch.
fn birth_age(is_burst: bool, dt: f32, rng: &mut u32) -> f32 {
    if is_burst {
        return 0.0;
    }
    rand01(rng) * dt.min(0.1)
}

/// What the sim asks of an emitter's owner: where it is, whether its model is drawn, and what it
/// rides. Never an emitter or child-draw entity, so disjoint from the emitters' transform writes.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct Owners<'w, 's> {
    transforms:
        Query<'w, 's, &'static GlobalTransform, (Without<ParticleEmitter>, Without<ChildDraw>)>,
    visibility: Query<'w, 's, &'static InheritedVisibility>,
    /// The transport the owner's model rides, if any: world-mode particles' stored frame.
    rides: crate::ride_frame::RideFrames<'w, 's>,
}

impl Owners<'_, '_> {
    /// The owner's live world position; a frozen emitter's stored anchor goes stale.
    fn at(&self, owner: Entity) -> Option<Vec3> {
        self.transforms.get(owner).ok().map(|gt| gt.translation())
    }

    /// Is the owner's model undrawn (last propagate's verdict)? An unreadable owner is not hidden.
    fn hidden(&self, owner: Entity) -> bool {
        matches!(self.visibility.get(owner), Ok(v) if !v.get())
    }
}

#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct SceneGates<'w, 's> {
    view: Res<'w, crate::view::ViewDistance>,
    exterior_windows: Res<'w, crate::wmo_portal::ExteriorWindows>,
    camera_claim: Res<'w, crate::wmo_portal::CameraInteriorClaim>,
    portals: Query<'w, 's, &'static crate::wmo_portal::WmoPortalInstance>,
    /// Buildings streamed in this frame, which can change a room admit.
    new_portals: Query<'w, 's, (), Changed<crate::wmo_portal::WmoPortalInstance>>,
}

impl SceneGates<'_, '_> {
    /// Any input of a draw-set verdict moved this frame.
    pub(crate) fn changed(&self) -> bool {
        self.view.is_changed()
            || self.exterior_windows.is_changed()
            || self.camera_claim.is_changed()
            || !self.new_portals.is_empty()
    }

    /// The far-clip wall, the exterior gate and the camera's placement for
    /// [`EmitterFade::in_draw_set`]; build once per walk.
    pub(crate) fn scene(
        &self,
        cam: Option<(&GlobalTransform, &Projection)>,
    ) -> (f32, crate::exterior_cull::ExteriorGate, Option<Entity>) {
        (
            self.view.farclip,
            crate::exterior_cull::ExteriorGate::build(&self.exterior_windows, cam),
            self.camera_claim.0.map(|c| c.room.instance),
        )
    }

    /// The room term: does the emitter's live placement admit its room?
    pub(crate) fn room_admits(&self, fade: &EmitterFade) -> bool {
        fade.room_admitted(
            fade.room
                .as_ref()
                .and_then(|r| self.portals.get(r.instance).ok()),
        )
    }
}

/// The water-plane interleave inputs ([`crate::sky_order::FAR_SIDE_BIAS`]): liquid surfaces, the
/// eye's submersion, and the rooms an anchor's liquid claim resolves from.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct WaterInterleave<'w, 's> {
    water: Query<'w, 's, &'static crate::liquid::WaterChunkInfo>,
    /// The spatial pre-filter every per-draw classification goes through.
    index: Res<'w, crate::liquid::WaterIndex>,
    underwater: Res<'w, crate::liquid::Underwater>,
    rooms: Query<'w, 's, &'static crate::wmo_portal::UnitWmoRoom>,
    placements: crate::liquid::RoomPlacements<'w, 's>,
    parents: Query<'w, 's, &'static ChildOf>,
}

impl WaterInterleave<'_, '_> {
    /// The eye's side of the water, which inverts every verdict (`far = above XOR submerged`).
    pub(crate) fn eye_submerged(&self) -> bool {
        self.underwater.0.any()
    }

    /// The loaded surfaces changed since this system last ran, so any verdict may have moved.
    pub(crate) fn surfaces_changed(&self) -> bool {
        self.index.is_changed()
    }
}

/// Is this draw on the eye's far side of its local water plane? The reference draws that side's
/// transparents before the water pass (dry eye: below; submerged: above; `0x4836d6`). Above is
/// `d ≥ −r` against the nearest admitted surface, and no surface counts as above (`+0x19c == 0`,
/// `0x707a10`). `r` is the lane's slack: the model's bound radius for emitters and ribbons
/// ([`model_far_side`]), 0 for meshes (`0x7079ed`).
pub(crate) fn far_side_of_water_at(
    w: &WaterInterleave,
    claim_seed: Option<Entity>,
    point_world: Vec3,
    r: f32,
) -> bool {
    let above = water_height(w, claim_seed, point_world).is_none_or(|d| is_above(d, r));
    if w.underwater.0.any() {
        above
    } else {
        !above
    }
}

/// `d = point − surface` in yd against the nearest admitted surface over the point, the claim
/// taken from `claim_seed`'s nearest ancestor with a room verdict; `None` without one.
pub(crate) fn water_height(
    w: &WaterInterleave,
    claim_seed: Option<Entity>,
    point_world: Vec3,
) -> Option<f32> {
    let wow = benilla_assets::coords::bevy_to_wow(point_world);
    // The spatial pre-filter first (a full walk per draw is too slow); no candidate over this
    // XY, the dry-land case, skips the room walk too.
    let candidates = w.index.over(wow[0], wow[1]);
    if candidates.is_empty() {
        return None;
    }
    let mut seed = claim_seed;
    let mut room = None;
    for _ in 0..8 {
        let Some(e) = seed else { break };
        if let Ok(rm) = w.rooms.get(e) {
            room = Some(rm);
            break;
        }
        seed = w.parents.get(e).ok().map(ChildOf::parent);
    }
    let claim = crate::liquid::unit_claim(room, &w.placements);
    let surfaces = candidates.iter().filter_map(|&e| w.water.get(e).ok());
    crate::liquid::surfaces_at(surfaces, wow, claim)
        .map(|z| wow[2] - z)
        .min_by(|a, b| a.abs().total_cmp(&b.abs()))
}

/// The mesh lane's test, at the batch's own transform with r = 0.
pub(crate) fn far_side_of_water(
    w: &WaterInterleave,
    claim_seed: Option<Entity>,
    anchor_world: Vec3,
) -> bool {
    far_side_of_water_at(w, claim_seed, anchor_world, 0.0)
}

/// The emitter and ribbon lanes' water side is the model's: the reference tests the plane once per
/// model at `world_matrix × bound centre` with slack `|row 0| × sphere radius`, and every emitter
/// reads that verdict (`0x7085fa`; ribbons `0x7081f1`). `model` is the instance root, `bound` its
/// [`benilla_assets::ModelEmitter::water_bound`]; with no matrix, r = 0 at `fallback_point`.
pub(crate) fn model_far_side(
    w: &WaterInterleave,
    model: Option<Entity>,
    model_gt: Option<&GlobalTransform>,
    bound: (Vec3, f32),
    fallback_point: Vec3,
) -> bool {
    match model_gt {
        Some(gt) => {
            let point = gt.transform_point(bound.0);
            let r = gt.affine().matrix3.x_axis.length() * bound.1;
            far_side_of_water_at(w, model, point, r)
        }
        None => far_side_of_water_at(w, model, fallback_point, 0.0),
    }
}

/// `d ≥ −r` (`0x7084cf`): a tie is above, NaN below.
fn is_above(d: f32, r: f32) -> bool {
    d >= -r
}

/// Does a booth-layered emitter freeze this frame (the camera's half of [`scene_frozen`])? A
/// sleeping booth camera freezes it, as the reference ticks an emitter only inside a draw; a
/// rate-throttled one ([`super::ViewThrottled`]) does not, its scene still running at full rate. A
/// draining pool never freezes, or it could never empty.
fn booth_frozen(cam_active: bool, throttled: bool, draining: bool) -> bool {
    !cam_active && !throttled && !draining
}

/// Does this emitter's scene freeze it: its booth camera, or its owner
/// ([`super::ParticleEmitter::set_frozen`]), since one tile-atlas camera draws many panes?
/// `booth` is `(cam_active, throttled)`, `None` in the world; draining overrides both.
fn scene_frozen(booth: Option<(bool, bool)>, owner_frozen: bool, draining: bool) -> bool {
    booth.is_some_and(|(cam_active, throttled)| booth_frozen(cam_active, throttled, draining))
        || (owner_frozen && !draining)
}

/// Per frame: emit, integrate and expand each emitter's pool into the shared stream.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
pub(super) fn simulate_particles(
    time: Res<Time>,
    tuning: Res<ParticleTuning>,
    // The draw-set gate's scene inputs: the far-clip wall, the exterior-window test (the camera's
    // own room exempt) and the portal PVS of the rooms inside a building.
    gates: SceneGates,
    interleave: WaterInterleave,
    mut commands: Commands,
    cam: Query<
        (
            Entity,
            Ref<GlobalTransform>,
            &Frustum,
            &Camera,
            Ref<Projection>,
            Option<Ref<Transform>>,
        ),
        With<WorldCamera>,
    >,
    owners: Owners,
    // Model-particle instances, hidden with their frozen emitter.
    mut child_draws: Query<
        (&mut Transform, &mut GlobalTransform, &mut Visibility),
        (
            With<ChildDraw>,
            Without<ParticleEmitter>,
            Without<WorldCamera>,
        ),
    >,
    images: Res<Assets<Image>>,
    mut quads: ResMut<EffectQuads>,
    // The ground-snap probe (file 0x2000): the walking collision geometry.
    spatial: SpatialQuery,
    owner_mul: OwnerMultipliers,
    // `Without<WorldCamera>` keeps the `&mut GlobalTransform` disjoint from the camera read.
    mut emitters: Query<
        (
            Entity,
            &mut ParticleEmitter,
            &mut Transform,
            &mut GlobalTransform,
            Option<Ref<EmitterFade>>,
            Option<&RenderLayers>,
            Option<&EffectLightOverride>,
            Option<&crate::interior::EmitterLitBy>,
        ),
        Without<WorldCamera>,
    >,
    // Booth cameras: a booth-layered emitter faces its own, matched by layer.
    booth_cams: Query<
        (
            Entity,
            &GlobalTransform,
            &RenderLayers,
            &Camera,
            Has<super::ViewThrottled>,
        ),
        (
            With<Camera3d>,
            Without<WorldCamera>,
            Without<ParticleEmitter>,
            Without<ChildDraw>,
        ),
    >,
    // A hosted emitter's sequence: its host's live `AnimationPlayer` slot and clip time.
    hosts: Query<(&AnimationPlayer, &benilla_assets::ModelAnimations)>,
    mut dumps: super::dumps::Dumps,
) {
    let Ok((world_cam, cam_tf, frustum, camera, projection, cam_local)) = cam.single() else {
        return;
    };
    // Clamped so a load hitch cannot fling every particle out in one step.
    let dt = time.delta_secs().min(0.1);
    if dt <= 0.0 {
        return;
    }
    // No draw-set input moved: a gated emitter's verdict stands, and its tests can be skipped.
    let gate_inputs_still = !cam_tf.is_changed()
        && !projection.is_changed()
        && !cam_local.as_ref().is_some_and(|l| l.is_changed())
        && !gates.changed()
        && !interleave.surfaces_changed();
    let cam_pos = cam_tf.translation();
    let density = tuning.density.clamp(0.25, 1.0);
    let snap_filter = crate::collision::WorldCollision::body_filter();
    let (_, cam_rot, _) = cam_tf.to_scale_rotation_translation();
    let cam_right = cam_rot * Vec3::X;
    let cam_up = cam_rot * Vec3::Y;
    // The far-clip axis the owner doodad's own cull uses, so the two cross the wall together.
    let cam_fwd = Vec3::from(cam_tf.forward());
    // Built once per walk, the same values the model visibility gate uses.
    let (farclip, exterior_gate, camera_instance) = gates.scene(Some((&*cam_tf, &*projection)));
    let dump_frame = dumps.depth_frame(time.elapsed_secs());
    let emit_dump = dumps.emit.due(time.elapsed_secs());

    for (
        entity,
        mut emitter,
        mut entity_tf,
        mut entity_global,
        fade,
        layers,
        light_override,
        lit_by,
    ) in &mut emitters
    {
        // The camera the quads face, the LOD measures from and the draw targets: a booth-layered
        // emitter's booth camera, else the world camera.
        let booth = layers
            .filter(|l| !l.intersects(&RenderLayers::default()))
            .and_then(|l| booth_cams.iter().find(|(_, _, cl, ..)| cl.intersects(l)));
        let is_booth = booth.is_some();
        // A frozen scene holds pool and age, pushes no quads, and hides model instances once.
        if scene_frozen(
            booth.map(|(_, _, _, c, throttled)| (c.is_active, throttled)),
            emitter.frozen,
            emitter.draining,
        ) {
            if !emitter.gated {
                emitter.gated = true;
                for slot in &emitter.model_instances {
                    for (e, _) in &slot.meshes {
                        if let Ok((_, _, mut cv)) = child_draws.get_mut(*e) {
                            *cv = Visibility::Hidden;
                        }
                    }
                }
            }
            continue;
        }
        if is_booth {
            emitter.gated = false;
        }
        let (draw_cam, e_cam_pos, e_right, e_up) = match booth {
            Some((cam_entity, tf, ..)) => {
                let (_, rot, _) = tf.to_scale_rotation_translation();
                (cam_entity, tf.translation(), rot * Vec3::X, rot * Vec3::Y)
            }
            None => (world_cam, cam_pos, cam_right, cam_up),
        };
        // The draw-set gate: the reference ticks an emitter only while its owner doodad is in the
        // frame's worklist, by frustum sphere and distance-fade cutoff (`0x683f80`; the sphere is
        // `[rec+0x68]`). Outside it pool and age freeze (`[obj+0x88]` accumulates nothing) and
        // resume with one frame's dt. The far-clip term is needed: our projection's far plane is
        // past `farclip`, and owners over `NEVER_FADE_RADIUS` never fade.
        if let Some(f) = fade.as_ref() {
            if emitter.gated && gate_inputs_still && !f.is_changed() {
                continue;
            }
            let in_set = f.in_draw_set(
                cam_pos,
                cam_fwd,
                farclip,
                // Lateral planes only: the depth bound is the farclip term.
                frustum.intersects_sphere(
                    &CullSphere {
                        center: f.center.into(),
                        radius: f.radius,
                    },
                    false,
                ),
                // From inside a WMO, a doodad outside is not in the worklist.
                f.exterior_admitted(&exterior_gate, camera_instance),
                // The window admits a whole building, not the sealed room the prop stands in.
                gates.room_admits(f),
            );
            if !in_set {
                // Frozen: nothing pushed is nothing drawn; model instances hide once.
                if !emitter.gated {
                    emitter.gated = true;
                    for slot in &emitter.model_instances {
                        for (e, _) in &slot.meshes {
                            if let Ok((_, _, mut cv)) = child_draws.get_mut(*e) {
                                *cv = Visibility::Hidden;
                            }
                        }
                    }
                }
                continue;
            }
            emitter.gated = false;
        } else if !is_booth && !emitter.draining {
            // Entity-owned emitters carry no `EmitterFade`, and vmangos streams transports
            // map-wide, so the far-clip wall applies to them too: the reference ticks particles
            // only in the animate step of a model the frame draws. Exempt: booth emitters (their
            // own camera), draining ones (a frozen pool never empties), a vanished owner (it
            // drains). The subject is the owner's live position.
            let subject = match emitter.owner {
                Some(o) => owners.at(o),
                None => Some(emitter.anchor_pos),
            };
            // And the owner's own draw verdict: an emitter is a world root, so a hide on its
            // model's hierarchy never reaches it.
            let owner_hidden = emitter.owner.is_some_and(|o| owners.hidden(o));
            if let Some(at) = subject {
                if owner_hidden || !crate::view::within_farclip(farclip, cam_pos, cam_fwd, at, 0.0)
                {
                    if !emitter.gated {
                        emitter.gated = true;
                        for slot in &emitter.model_instances {
                            for (e, _) in &slot.meshes {
                                if let Ok((_, _, mut cv)) = child_draws.get_mut(*e) {
                                    *cv = Visibility::Hidden;
                                }
                            }
                        }
                    }
                    continue;
                }
                emitter.gated = false;
            }
        }
        let ParticleEmitter {
            def,
            ride,
            placement,
            owner,
            on_owner_loss,
            draining,
            alpha_src,
            alpha,
            anchor,
            anchor_pos,
            particles,
            accumulator,
            emitter_prev,
            inherit_accum,
            inherit_vel,
            gate_prev,
            age,
            host,
            seq,
            rng,
            owner_reach,
            size_scale,
            water_bound,
            texture,
            recursion: _,
            light_node: _,
            children,
            geometry: _,
            model_instances,
            gated: _,
            frozen: _,
            clip,
        } = &mut *emitter;
        // The water test's model frame, before `anchor` is shadowed: the instance root. A joint
        // owner only seeds the room walk, as a joint's transform is not the instance matrix.
        let water_model = (*anchor).or(*owner);
        let water_gt = (*anchor).and_then(|e| owners.transforms.get(e).ok().copied());
        let water_bound = *water_bound;
        *age += dt;
        let (clock_seq, elapsed_s) = match *host {
            Some(h) => match hosts
                .get(h)
                .ok()
                .and_then(|(p, a)| crate::doodad_anim::playing_seq(p, a))
            {
                Some((s, t)) => {
                    *seq = Some(s);
                    (Some(s), t)
                }
                None => (*seq, 0.0),
            },
            None => (*seq, *age),
        };
        // The global-sequence cursor: the spawn age is `sceneNow − attach`.
        let gseq_now = f64::from(*age);
        // `anchored` is world mode (file flag `0x10` clear); see [`Particle`].
        let anchored = !def.model_space();

        // 0. Track a streamed owner's transform; once it is gone, free or drain per
        //    `on_owner_loss`.
        if let Some(o) = *owner {
            match owners.transforms.get(o) {
                Ok(gt) => *placement = gt.compute_transform(),
                Err(_) => {
                    *owner = None;
                    match *on_owner_loss {
                        // The reference frees a model's emitters in its destructor: nothing drains.
                        OwnerLoss::Free => {
                            for slot in model_instances.iter() {
                                for (e, _) in &slot.meshes {
                                    commands.entity(*e).despawn();
                                }
                            }
                            commands.entity(entity).despawn();
                            continue;
                        }
                        OwnerLoss::Drain => {
                            *draining = true;
                            // The pool lives out its lifespans in place, like a missile's impact.
                            if !particles.is_empty() {
                                debug!(
                                    "fx orphan: emitter {entity} ({}) lost its owner with {} \
                                     live particles frozen at {:?}",
                                    texture
                                        .path()
                                        .map_or_else(|| "<no path>".into(), |p| p.to_string()),
                                    particles.len(),
                                    *anchor_pos
                                );
                            }
                        }
                    }
                }
            }
        }
        if *draining && particles.is_empty() && children.iter().all(|c| c.particles.is_empty()) {
            for slot in model_instances.iter() {
                for (e, _) in &slot.meshes {
                    commands.entity(*e).despawn();
                }
            }
            commands.entity(entity).despawn();
            continue;
        }
        // Dormant: nothing live, nothing draining, not emitting at this clock. Judged after the
        // owner-loss and drain blocks and the clock, which must run every frame.
        if particles.is_empty()
            && !*draining
            && children.iter().all(|c| c.particles.is_empty())
            && !def.timing.emitting(clock_seq, elapsed_s, gseq_now)
        {
            continue;
        }
        // The model's render alpha (`emitter+0x1a8 = Model+0x19c`, `0x718960` at `0x719073`): an
        // entity cloud's composed model alpha, or a placed doodad's distance fade.
        *alpha = alpha_src.map_or(1.0, |e| owner_mul.alpha(e))
            * fade.as_ref().map_or(1.0, |f| f.distance_alpha(cam_pos));
        // The model's live translation, or the last known while the pool drains.
        match *anchor {
            Some(a) => {
                if let Ok(gt) = owners.transforms.get(a) {
                    *anchor_pos = gt.translation();
                }
            }
            None if owner.is_none() => *anchor_pos = placement.translation,
            None => {} // joint-owned, unanchored (placed doodads): the spawn placement stands
        }
        // `$WOW_EMIT_DUMP`'s model, taken before `anchor` is shadowed below.
        let dump_owner = (*host).or(*anchor);

        // 0a. The ride frame (`[CM2Model+0x17c]`): the transport the model stands on, inherited
        //     down the `ParentModel` chain; one that streamed out reads as none.
        let deck = anchor
            .or(*owner)
            .and_then(|model| owners.rides.source(model))
            .and_then(|t| {
                owners
                    .transforms
                    .get(t)
                    .ok()
                    .map(|gt| (t, crate::ride_frame::ride_matrix(gt)))
            })
            // Model mode ignores the ride frame (`0x7b518e`, `0x7b3ef9`): it rides its host.
            .filter(|_| anchored);
        // Boarding and leaving re-express every live particle (`0x7187f0` → `0x7b5e60`); a
        // moving deck folds nothing, the draw's live `A` carries it.
        if let Some(fold) = ride.retarget(deck) {
            let rot = Quat::from_mat3a(&fold.matrix3);
            for p in particles
                .iter_mut()
                .chain(children.iter_mut().flat_map(|c| &mut c.particles))
            {
                p.pos = fold.transform_point3(p.pos);
                p.vel = rot * p.vel;
            }
            // The emitter-motion state crosses too, or the frame change reads as motion; whether
            // the reference's `0x7b5e60` re-expresses `rt+0x248` is untraced.
            *emitter_prev = emitter_prev.map(|prev| fold.transform_point3(prev));
            *inherit_vel = rot * *inherit_vel;
        }
        // Births bake through `rt+0x1fc = srcMx · A⁻¹` (`0x7b51b0`), so the stored-frame terms
        // below are deck-relative; `placement` stays world for the LOD, water side and model draw.
        let emit_place = match ride.matrix() {
            Some(a) => {
                let inv = a.inverse();
                Transform {
                    translation: inv.transform_point3(placement.translation),
                    rotation: Quat::from_mat3a(&inv.matrix3) * placement.rotation,
                    scale: placement.scale,
                }
            }
            None => *placement,
        };

        // 0b. The emitter-motion terms feed off the origin's one-frame Δ in the stored frame
        //     (`rt+0x22c`, the translation row of `rt+0x1fc`), refreshed every frame (`rt+0x248`,
        //     `0x7b5265`). Deviation: in model mode the reference adds the raw world vector to
        //     local coordinates; we fold it into the local frame, because our world axes are
        //     Bevy's, not WoW's.
        let emitter_world = emit_place.transform_point(wow_to_bevy(def.position));
        let emitter_delta = emitter_prev.map_or(Vec3::ZERO, |prev| emitter_world - prev);
        *emitter_prev = Some(emitter_world);
        // R(+Z, 90°) is applied at emission, so stored vectors are post-R and these folds R-free;
        // the reference folds it in its draw matrix, an equivalent composition.
        // World to the stored frame: the identity in world mode.
        let to_stored = |world: Vec3, placement: &Transform| {
            if anchored {
                world
            } else {
                Vec3::from(bevy_to_wow(
                    (placement.rotation.inverse() * world) / placement.scale.max(Vec3::splat(1e-6)),
                ))
            }
        };
        let fraction = follow_fraction(def, emitter_delta.length() / dt);
        let follow = if fraction == 0.0 || emitter_delta == Vec3::ZERO {
            Vec3::ZERO
        } else {
            to_stored(fraction * emitter_delta, &emit_place)
        };
        // Velocity inherit (file 0x40): the 30 Hz trigger, zeroed with nothing live (rt+0x64).
        if def.inherits_emitter_motion() {
            inherit_trigger(
                inherit_accum,
                inherit_vel,
                dt,
                emitter_delta,
                !particles.is_empty(),
                def.inherit_scale,
            );
        }

        // Every track samples per frame on the current sequence's clock (`0x714260`): a hosted
        // emitter's host's playing sequence and clip time, else its slot on the spawn-age clock.
        // Births read the frame's values (Frost Nova's radius grows with its ring).
        let now = def.params.sample(clock_seq, elapsed_s, gseq_now);

        // 1. Integrate the live pool ([`integrate_particle`]): up is WoW +Z in model mode and Bevy
        //    +Y in world mode; gravity is read live each frame, as the reference does.
        let g = now.gravity;
        let drag = def.drag;
        // Kill-outbound: the emitter's live origin in the stored frame, composed like a birth.
        let kill_origin = def.kill_outbound().then(|| {
            if anchored {
                emitter_world
            } else {
                Vec3::from(def.position)
            }
        });
        let env = StepEnv {
            dt,
            gravity: g,
            drag,
            anchored,
            kill_origin,
            follow,
        };
        particles.retain_mut(|p| integrate_particle(p, &env));

        // 2. Emit: [`accumulate_emission`] owes births from a continuous pour or a burst's rising
        //    edge (`0x718ec8`), at the rate track step-sampled on the clip clock; the enabled track
        //    (file +0x1dc) gates new births only. The reference's one particle distance mechanism
        //    is this emission LOD (`0x7b5550`): × clamp(1 − (camDist − 50)·0.02, 0.25, 1), then
        //    × `particleDensity`.
        let emitting = !*draining && def.timing.emitting(clock_seq, elapsed_s, gseq_now);
        let rate = def.timing.rate(clock_seq, elapsed_s, gseq_now);
        let dist_lod =
            (1.0 - (placement.translation.distance(e_cam_pos) - 50.0) * 0.02).clamp(0.25, 1.0);
        let burst = accumulate_emission(
            def.burst(),
            rate,
            emitting,
            density * dist_lod,
            dt,
            accumulator,
            gate_prev,
        );
        if burst > 0.0 && benilla_assets::trace::enabled() {
            benilla_assets::trace::line("fx", &format!("burst n={burst} t={elapsed_s:.2}s"));
        }
        while *accumulator >= 1.0 && particles.len() < MAX_PARTICLES {
            *accumulator -= 1.0;
            let (base, dir) = emit_local(def, &now, rng);
            let speed =
                now.emission_speed * (1.0 + now.speed_variation * (rand01(rng) * 2.0 - 1.0));
            // World mode (`0x10` clear) bakes a birth into world through the live emitter matrix
            // (`0x7b8b0f` → `0x7bca80`, `0x7b8acf` → `0x7bcb40`); model mode stores it local and
            // the draw re-applies the matrix (`0x7b8aa5`, `0x7b3efb`).
            let (mut pos, vel) = if anchored {
                (
                    emit_place.transform_point(wow_to_bevy(base.to_array())),
                    emit_place.rotation
                        * (emit_place.scale * wow_to_bevy((dir * speed).to_array())),
                )
            } else {
                (base, dir * speed)
            };
            // Ground snap (file 0x2000): at birth, in world mode, a 20 yd probe down against the
            // walking collision (the reference's flags `0x100111`) sets a hit particle on the
            // surface, lifted by its birth size.
            if anchored && def.ground_snap() {
                // Probed at the world point and stored back; a yaw-only `A` keeps world down.
                let world = ride.to_world(pos);
                if let Some(hit) = spatial.cast_ray(world, Dir3::NEG_Y, 20.0, true, &snap_filter) {
                    pos = ride.to_stored(
                        world.with_y(world.y - hit.distance + def.over_life.sample(0.0).size),
                    );
                }
            }
            // Inherit (the kernels' closing block): `vel += (1 + S11·speedVariation) · inherit`.
            let vel = if def.inherits_emitter_motion() && *inherit_vel != Vec3::ZERO {
                vel + (1.0 + now.speed_variation * rand_s11(rng))
                    * to_stored(*inherit_vel, &emit_place)
            } else {
                vel
            };
            // Model particles (`0x7b2420`): orientation from the birth basis; the tumble's X is
            // `min + u·range`, but Y and Z are a [1, 2) mantissa times the range, their min
            // unused, as the reference rolls them.
            let (quat, angvel) = if def.geometry_model.is_some() {
                let amin = def.angular_velocity_min;
                let amax = def.angular_velocity_max;
                let mut w = [
                    amin[0] + rand01(rng) * (amax[0] - amin[0]),
                    (1.0 + rand01(rng)) * (amax[1] - amin[1]),
                    (1.0 + rand01(rng)) * (amax[2] - amin[2]),
                ];
                if def.tumble_random_sign() {
                    for a in &mut w {
                        if next_u32(rng) & 1 == 0 {
                            *a = -*a;
                        }
                    }
                }
                // R(+Z, 90°) is a +90° turn about Bevy +Y, appended to the frame rotation.
                let r90 = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
                let quat = if anchored {
                    emit_place.rotation * r90
                } else {
                    r90
                };
                (quat, wow_to_bevy(w))
            } else {
                (Quat::IDENTITY, Vec3::ZERO)
            };
            let phase = next_u32(rng);
            particles.push(Particle {
                pos,
                vel,
                age: birth_age(def.burst(), dt, rng),
                life: now.lifespan,
                phase,
                fresh: true,
                quat,
                angvel,
            });
        }

        // 2b. Children (`0x7b5550`) drive off the post-birth pool on their own tracks, slot 0 on
        //     the parent's age; a draining parent stops driving them.
        for child in children.iter_mut() {
            // A child cloud is its own fresh instance: its gseq cursor is its own age.
            let c_emitting = !*draining && child.def.timing.emitting(None, *age, f64::from(*age));
            let c_rate = child.def.timing.rate(None, *age, f64::from(*age));
            let c_now = child.def.params.sample(None, *age, f64::from(*age));
            drive_child(
                child,
                &c_now,
                particles,
                c_rate,
                c_emitting,
                density * dist_lod,
                dt,
                anchored,
                &emit_place,
            );
            let c_env = StepEnv {
                dt,
                gravity: c_now.gravity,
                drag: child.def.drag,
                anchored,
                kill_origin: None,
                follow: Vec3::ZERO,
            };
            child
                .particles
                .retain_mut(|p| integrate_particle(p, &c_env));
        }

        // 3. Expand each pool once its texture is resident, never the fallback. The anchor is
        //    the draw's sort point, also published on the entity for the instruments.
        let anchor = if anchored {
            *anchor_pos
        } else {
            placement.translation
        };
        entity_tf.translation = anchor;
        // Written directly for same-frame readers: we run after propagation, at the world root.
        *entity_global = GlobalTransform::from(*entity_tf);
        let frame = DrawFrame {
            anchored,
            alpha: *alpha,
            ride: *ride,
            size_scale: *size_scale,
        };
        let cam = CamBasis {
            right: e_right,
            up: e_up,
        };
        // A geometry emitter draws instances ([`super::model`]), never quads.
        let want_quads = def.geometry_model.is_none() && images.contains(&*texture);
        // One rung for every effect of a model, plus the water interleave: a cloud on the eye's
        // far side sorts under the water pass, classified once per model (`0x7084a0`).
        let far = !is_booth
            && model_far_side(
                &interleave,
                water_model,
                water_gt.as_ref(),
                water_bound,
                placement.translation,
            );
        // Trace tag `fx`: which clouds sorted under the water pass this frame.
        if far && !particles.is_empty() && benilla_assets::trace::enabled() {
            benilla_assets::trace::line(
                "fx",
                &format!(
                    "far-side cloud at=[{:.1},{:.1},{:.1}] n={}",
                    placement.translation.x,
                    placement.translation.y,
                    placement.translation.z,
                    particles.len()
                ),
            );
        }
        let bias = super::owner_last_bias(*owner_reach)
            + if far {
                crate::sky_order::FAR_SIDE_BIAS
            } else {
                0.0
            };
        let start = quads.begin();
        if want_quads && !particles.is_empty() {
            expand_quads(def, particles, &frame, placement, &cam, &mut quads.verts);
        }
        // `$WOW_PARTICLE_DEPTHDUMP` over the quads just written; booths draw to their own target.
        if let Some(fidx) = dump_frame {
            if !is_booth
                && def.geometry_model.is_none()
                && !particles.is_empty()
                && super::depthdump::bone_selected(u32::from(def.bone))
            {
                super::depthdump::dump_emitter(
                    fidx,
                    def,
                    particles,
                    &frame,
                    placement,
                    // The world point: `emitter_world` is in the stored frame.
                    ride.to_world(emitter_world),
                    &cam,
                    &cam_tf,
                    camera,
                    &projection,
                    images.contains(&*texture),
                    &quads.verts[start as usize..],
                );
            }
        }
        // `$WOW_EMIT_DUMP`, here so `live` counts this frame's births.
        if emit_dump {
            dumps.emit.dump(
                dump_owner,
                &super::emitdump::Decision {
                    def,
                    seq: clock_seq,
                    elapsed: elapsed_s,
                    rate,
                    emitting,
                    live: particles.len(),
                    now: &now,
                    at: emitter_world,
                },
            );
        }
        // The model's committed light, shared by its child draws: one light node per model.
        let committed = owner_mul.committed_light(lit_by);
        let lighting = fold_committed_light(&mut quads.verts[start as usize..], def.lit, committed);
        quads.commit_quads(
            start,
            EffectDrawSpec {
                cam: draw_cam,
                texture: texture.id(),
                blend: def.blend.into(),
                fog: EffectFog::for_blend(def.flags, def.blend),
                lighting,
                anchor,
                bias,
                raster_bias: 0,
                raster_slope: 0.0,
                cam_relative: false,
                no_depth_test: false,
                main_entity: entity,
                light: light_override.map(|l| l.0.clone()),
                // A UI model tile's pane cell (`set_clip`); `None` in the world.
                clip: *clip,
            },
        );
        // Child pools: their own texture, blend and fog, the parent's anchor and rung.
        for child in children.iter() {
            if child.particles.is_empty() || !images.contains(&child.texture) {
                continue;
            }
            let cstart = quads.begin();
            expand_quads(
                &child.def,
                &child.particles,
                &frame,
                placement,
                &cam,
                &mut quads.verts,
            );
            let child_lighting = fold_committed_light(
                &mut quads.verts[cstart as usize..],
                child.def.lit,
                committed,
            );
            quads.commit_quads(
                cstart,
                EffectDrawSpec {
                    cam: draw_cam,
                    texture: child.texture.id(),
                    blend: child.def.blend.into(),
                    fog: EffectFog::for_blend(child.def.flags, child.def.blend),
                    // Its own record's lighting verdict, under the parent's light node.
                    lighting: child_lighting,
                    anchor,
                    bias,
                    raster_bias: 0,
                    raster_slope: 0.0,
                    cam_relative: false,
                    no_depth_test: false,
                    main_entity: entity,
                    light: light_override.map(|l| l.0.clone()),
                    // The parent's cell and clip.
                    clip: *clip,
                },
            );
        }
    }
}

/// The two per-model multipliers a cloud takes from its owner: render alpha and committed light.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct OwnerMultipliers<'w, 's> {
    /// The per-model render alpha, composed along the attached-model chain.
    alphas: crate::model_fade::ModelAlphas<'w, 's>,
    /// A lit emitter's room light node inside a WMO, never the sun (`0x6a7300`); absent outdoors.
    lights: Query<'w, 's, &'static crate::interior::ParticleLight>,
}

impl OwnerMultipliers<'_, '_> {
    /// `instance`'s composed render alpha.
    fn alpha(&self, instance: Entity) -> f32 {
        self.alphas.get(instance)
    }

    /// The committed light of the node this emitter registered under, if it has one.
    fn committed_light(&self, lit_by: Option<&crate::interior::EmitterLitBy>) -> Option<[f32; 3]> {
        self.lights.get(lit_by?.0).ok().map(|l| l.0)
    }
}

/// Fold a lit emitter's committed light into the RGB it just pushed and return its lighting: one
/// RGB per draw, multiplied in the shader's own gamma space. Without one the shader lights it.
fn fold_committed_light(
    verts: &mut [EffectVertex],
    lit: bool,
    committed: Option<[f32; 3]>,
) -> EffectLighting {
    match (lit, committed) {
        (false, _) => EffectLighting::None,
        (true, None) => EffectLighting::Scene,
        (true, Some(mul)) => {
            for v in verts {
                for (c, m) in v.color.iter_mut().zip(mul) {
                    // Clamp the product, not the light, which the reference leaves unclamped
                    // (`0x71c2f0`, `0x4549a0`): GL clamps the lit vertex colour.
                    *c = (*c * m).clamp(0.0, 1.0);
                }
            }
            EffectLighting::Committed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        birth_age, booth_frozen, fold_committed_light, follow_fraction, inherit_trigger,
        integrate_particle, is_above, scene_frozen, ChildEmitter, EffectLighting, EffectVertex,
        Particle, StepEnv, Vec3,
    };
    use bevy::prelude::{Quat, Transform};

    /// The burst loop passes `w = 0` (`0x7b5600`): the whole puff ages as one.
    #[test]
    fn a_burst_births_every_particle_at_age_zero() {
        let mut rng = 0x1234_5678;
        for _ in 0..64 {
            assert_eq!(birth_age(true, 0.016, &mut rng), 0.0);
        }
        // Not a function of dt: a hitch does not age a burst either.
        assert_eq!(birth_age(true, 5.0, &mut rng), 0.0);
    }

    #[test]
    fn a_poured_particle_is_born_aged_within_its_own_frame() {
        let mut rng = 0x9e37_79b9;
        let dt = 0.016;
        let mut saw_nonzero = false;
        for _ in 0..256 {
            let age = birth_age(false, dt, &mut rng);
            assert!((0.0..dt).contains(&age), "age {age} outside [0, {dt})");
            saw_nonzero |= age > 0.0;
        }
        assert!(saw_nonzero, "the seed is a real draw, not a constant 0");
    }

    /// `birth_age`'s clamp stands in for the reference's 0.1 s substep.
    #[test]
    fn a_hitch_frame_cannot_birth_a_particle_older_than_the_substep_cap() {
        let mut rng = 0xdead_beef;
        for _ in 0..256 {
            let age = birth_age(false, 5.0, &mut rng);
            assert!((0.0..0.1).contains(&age), "age {age} outside [0, 0.1)");
        }
    }

    fn one_quad() -> Vec<EffectVertex> {
        vec![
            EffectVertex {
                pos: [0.0; 3],
                uv: [0.0; 2],
                color: [0.8, 0.4, 0.2, 0.5],
            };
            4
        ]
    }

    /// Alpha is never lit: it is the blend weight, and the additive `rgb·α` would square the light.
    #[test]
    fn a_committed_light_folds_into_the_rgb_and_leaves_the_blend_weight_alone() {
        let mut verts = one_quad();
        assert_eq!(
            fold_committed_light(&mut verts, false, Some([0.5, 0.5, 0.5])),
            EffectLighting::None,
            "an UNLIT emitter takes no light at all, committed or not"
        );
        assert_eq!(verts[0].color, [0.8, 0.4, 0.2, 0.5]);

        assert_eq!(
            fold_committed_light(&mut verts, true, None),
            EffectLighting::Scene,
            "lit with no node of its own is the exterior lane — the shader's, per view"
        );
        assert_eq!(verts[0].color, [0.8, 0.4, 0.2, 0.5], "and it folds nothing");

        assert_eq!(
            fold_committed_light(&mut verts, true, Some([0.5, 0.25, 0.0])),
            EffectLighting::Committed,
            "lit under a light node folds that node's constant in"
        );
        for v in &verts {
            assert_eq!(v.color, [0.4, 0.1, 0.0, 0.5]);
        }
    }

    /// `0x7084cf`: a tie at `d == −r` is above, NaN below; r = 0 is the sign test, 0 above.
    #[test]
    fn membership_is_d_at_least_minus_r() {
        // A model bobbing within its radius of the plane stays above.
        assert!(is_above(-0.5, 2.0));
        assert!(is_above(-2.0, 2.0), "tie at d == -r lands above");
        assert!(!is_above(-2.1, 2.0));
        // r = 0 (the mesh lane): the sign test, 0 above.
        assert!(is_above(0.0, 0.0));
        assert!(!is_above(-0.1, 0.0));
        // NaN is below, the reference's unordered-compare branch.
        assert!(!is_above(f32::NAN, 2.0));
    }

    /// A throttled camera skips renders of a scene still posing at full rate: no freeze.
    #[test]
    fn a_throttled_booth_camera_does_not_freeze_its_emitters() {
        // Asleep: the pane closed, the reference's cull.
        assert!(booth_frozen(false, false, false));
        // Paced, on a skipped frame: not frozen.
        assert!(!booth_frozen(false, true, false));
        // Paced, on a drawn frame.
        assert!(!booth_frozen(true, true, false));
        assert!(!booth_frozen(true, false, false));
        // A draining pool runs out on any camera.
        assert!(!booth_frozen(false, false, true));
        assert!(!booth_frozen(false, true, true));
    }

    /// The tile atlas's one camera stays active while any cell is packed, so only the owner can
    /// freeze a pane that left the paint list.
    #[test]
    fn an_owner_freezes_a_cloud_whose_camera_still_says_drawn() {
        // The tile atlas's own shape: the camera is up, and the pane is not.
        assert!(!scene_frozen(Some((true, false)), false, false), "drawing");
        assert!(
            scene_frozen(Some((true, false)), true, false),
            "the owner parked the pane — the camera cannot see that"
        );
        // A booth camera still freezes its own scene.
        assert!(scene_frozen(Some((false, false)), false, false));
        assert!(
            !scene_frozen(Some((false, true)), false, false),
            "…and a throttled camera is awake (1559)"
        );
        // A world-lane emitter has no booth camera; the owner still reaches it.
        assert!(!scene_frozen(None, false, false));
        assert!(scene_frozen(None, true, false));
        // Draining overrides both.
        assert!(!scene_frozen(Some((false, false)), true, true));
        assert!(!scene_frozen(None, true, true));
    }

    fn particle(pos: Vec3, vel: Vec3) -> Particle {
        Particle {
            pos,
            vel,
            age: 0.0,
            life: 10.0,
            phase: 0,
            fresh: false,
            quat: Quat::IDENTITY,
            angvel: Vec3::ZERO,
        }
    }

    fn env(kill_origin: Option<Vec3>, follow: Vec3) -> StepEnv {
        StepEnv {
            dt: 0.1,
            gravity: 0.0,
            drag: 0.0,
            anchored: true,
            kill_origin,
            follow,
        }
    }

    /// `0x7b2680`: the pre-drag step velocity against the updated position.
    #[test]
    fn kill_outbound_dies_at_the_centre_crossing() {
        let origin = Vec3::new(1.0, 2.0, 3.0);
        // Inward at 2 yd/s, 0.5 yd out: crosses at t = 0.25 s.
        let mut p = particle(origin + Vec3::X * 0.5, -Vec3::X * 2.0);
        let kill = env(Some(origin), Vec3::ZERO);
        assert!(integrate_particle(&mut p, &kill), "0.3 yd out, inbound");
        assert!(integrate_particle(&mut p, &kill), "0.1 yd out, inbound");
        assert!(
            !integrate_particle(&mut p, &kill),
            "crossed to −0.1 yd: motion now points away — dead"
        );
        // Same trajectory, no kill origin: sails straight through.
        let free_env = env(None, Vec3::ZERO);
        let mut free = particle(origin + Vec3::X * 0.5, -Vec3::X * 2.0);
        for _ in 0..5 {
            assert!(integrate_particle(&mut free, &free_env));
        }
        // An outbound particle dies on its first step; no authored emitter does this.
        let mut out = particle(origin + Vec3::X * 0.5, Vec3::X);
        assert!(!integrate_particle(&mut out, &kill));
    }

    /// Flag `0x10` sets the ride-vs-trail baseline (`0x7b8a9a`); `0x4000` adds the response.
    #[test]
    fn follow_fraction_is_the_authored_response_and_nothing_else() {
        let plain = crate::particles::tests::plain_def(); // no flags
        assert_eq!(
            follow_fraction(&plain, 30.0),
            0.0,
            "no 0x4000: no per-frame term at any speed — ride-vs-trail is the storage space, not \
             a correction (1585)"
        );
        let riding = benilla_formats::ParticleEmitterDef {
            flags: 0x0011, // the carried torch, Club_1H_Torch_A_01.m2
            ..crate::particles::tests::plain_def()
        };
        assert_eq!(
            follow_fraction(&riding, 30.0),
            0.0,
            "file 0x10 alone adds NO term: model mode's draw already re-applies the live emitter \
             matrix, which is the whole 100% ride"
        );
        // ArcaneShot's authored pair: 0.1 at 2.5 yd/s, 0.9 at 16.667.
        let following = benilla_formats::ParticleEmitterDef {
            flags: 0x4000,
            follow_speed1: 2.5,
            follow_scale1: 0.1,
            follow_speed2: 16.667,
            follow_scale2: 0.9,
            ..crate::particles::tests::plain_def()
        };
        assert!((follow_fraction(&following, 2.5) - 0.1).abs() < 1e-3);
        assert_eq!(
            follow_fraction(&following, 40.0),
            1.0,
            "clamped: on a fast missile the head glow rides its emitter exactly"
        );
        // Equal authored speeds: the reference zeroes both, so nothing is added.
        let degenerate = benilla_formats::ParticleEmitterDef {
            flags: 0x4000,
            follow_speed1: 4.0,
            follow_speed2: 4.0,
            ..crate::particles::tests::plain_def()
        };
        assert_eq!(follow_fraction(&degenerate, 30.0), 0.0);
    }

    /// The world-mode draw never folds `rt+0x1fc` back in (`0x7b3f48`).
    #[test]
    fn a_world_mode_particle_ignores_everything_its_host_does_after_birth() {
        use crate::particles::quads::{particle_center, DrawFrame};
        let born_at = Vec3::new(12.0, 3.0, -40.0);
        let p = particle(born_at, Vec3::ZERO);
        let frame = DrawFrame {
            anchored: true, // 0x10 clear: the world store
            alpha: 1.0,
            ride: crate::ride_frame::StoredFrame::default(), // on the ground: no fold
            size_scale: 1.0,
        };
        // Every host pose we can think of, including ones no bone reaches.
        for placement in [
            Transform::IDENTITY,
            Transform::from_translation(Vec3::new(500.0, -20.0, 900.0)),
            Transform::from_rotation(Quat::from_rotation_y(std::f32::consts::PI)),
            Transform::from_rotation(Quat::from_euler(bevy::math::EulerRot::YXZ, 1.2, -0.7, 2.9))
                .with_translation(Vec3::new(-7.0, 44.0, 3.0))
                .with_scale(Vec3::splat(3.5)),
        ] {
            assert_eq!(
                particle_center(&frame, &placement, &p),
                born_at,
                "a world-mode particle moved when its host did"
            );
        }
    }

    /// The store is the deck's frame, drawn through the deck's live pose (`A · T · S`, `0x7b3f4f`).
    #[test]
    fn a_world_mode_particle_rides_the_transport_under_it() {
        use crate::particles::quads::{particle_center, DrawFrame};
        use bevy::math::Affine3A;
        let mut ride = crate::ride_frame::StoredFrame::default();
        let deck = bevy::prelude::Entity::from_raw_u32(7).expect("a valid test entity id");
        let lift_low = Affine3A::from_translation(Vec3::new(-1280.0, 60.0, 185.0));
        // Board at the bottom of the shaft, then ride 30 yd up.
        let fold = ride
            .retarget(Some((deck, lift_low)))
            .expect("boarding folds");
        let born_at = Vec3::new(-1279.0, 61.5, 186.0);
        let p = particle(fold.transform_point3(born_at), Vec3::ZERO);
        let placement = Transform::IDENTITY; // model mode's input; world mode must ignore it
        let mut frame = DrawFrame {
            anchored: true,
            alpha: 1.0,
            ride,
            size_scale: 1.0,
        };
        assert_eq!(
            particle_center(&frame, &placement, &p),
            born_at,
            "boarding moved a particle that was standing still"
        );
        let lift_high = Affine3A::from_translation(Vec3::new(-1280.0, 90.0, 185.0));
        assert!(
            ride.retarget(Some((deck, lift_high))).is_none(),
            "the deck MOVING must not re-express the store — only boarding and leaving do"
        );
        frame.ride = ride;
        assert_eq!(
            particle_center(&frame, &placement, &p),
            born_at + Vec3::new(0.0, 30.0, 0.0),
            "the particle did not ride the lift up"
        );
    }

    #[test]
    fn a_model_mode_particle_rides_its_host_rigidly() {
        use crate::particles::quads::{particle_center, DrawFrame};
        let p = particle(Vec3::new(1.0, 0.0, 0.0), Vec3::ZERO); // WoW axes, 1 yd out on +X
        let frame = DrawFrame {
            anchored: false, // 0x10 set: the emitter-local store
            alpha: 1.0,
            ride: crate::ride_frame::StoredFrame::default(),
            size_scale: 1.0,
        };
        let moved = Transform::from_translation(Vec3::new(10.0, 0.0, 0.0));
        assert_eq!(
            particle_center(&frame, &moved, &p) - particle_center(&frame, &Transform::IDENTITY, &p),
            Vec3::new(10.0, 0.0, 0.0),
            "model mode follows its host's translation in full"
        );
        let turned = Transform::from_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
        assert!(
            particle_center(&frame, &turned, &p).distance(particle_center(
                &frame,
                &Transform::IDENTITY,
                &p
            )) > 1.0,
            "model mode follows its host's ROTATION too — the one thing world mode must not do"
        );
    }

    /// `0x7b2744`: the fresh bit (particle+0xd) skips the first integrate's add.
    #[test]
    fn follow_delta_skips_only_the_first_integrate() {
        let following = env(None, Vec3::X * 0.5);
        let mut p = particle(Vec3::ZERO, Vec3::ZERO);
        p.fresh = true;
        assert!(integrate_particle(&mut p, &following));
        assert_eq!(
            p.pos,
            Vec3::ZERO,
            "first integrate: fresh bit eaten, no add"
        );
        assert!(integrate_particle(&mut p, &following));
        assert_eq!(
            p.pos,
            Vec3::X * 0.5,
            "second integrate: the shared delta applies"
        );
    }

    /// `0x7b5230`: held `oneFrameΔ·((1/30)/accum)·scale` past 1/30 s, zero with nothing live.
    #[test]
    fn inherit_trigger_fires_at_thirty_hertz_with_the_exact_factor() {
        let (mut accum, mut held) = (0.0, Vec3::ZERO);
        let delta = Vec3::X * 0.1; // one frame's emitter motion
        inherit_trigger(&mut accum, &mut held, 0.02, delta, true, 6.0);
        assert_eq!(
            held,
            Vec3::ZERO,
            "0.02 s accumulated: below the 1/30 window"
        );
        inherit_trigger(&mut accum, &mut held, 0.02, delta, true, 6.0);
        // accum 0.04 > 1/30: held = 0.1·((1/30)/0.04)·6 = 0.5 on x.
        assert!((held.x - 0.5).abs() < 1e-4, "the (1/30)/accum·scale factor");
        assert_eq!(accum, 0.0, "trigger resets the accumulator");
        // Between triggers the held value stands even as the emitter stops moving.
        inherit_trigger(&mut accum, &mut held, 0.02, Vec3::ZERO, true, 6.0);
        assert!((held.x - 0.5).abs() < 1e-4, "held between triggers");
        // A trigger with nothing live zeroes it (the rt+0x64 gate).
        inherit_trigger(&mut accum, &mut held, 0.04, delta, false, 6.0);
        assert_eq!(held, Vec3::ZERO);
    }

    /// `0x7b5b9f`: one `rate·dt` per live parent particle, born at it with its velocity (0x40).
    #[test]
    fn child_drive_scales_with_the_parent_pool() {
        let mut child = ChildEmitter::bare(benilla_formats::ParticleEmitterDef {
            flags: 0x40, // inherit the parent particle's velocity
            ..crate::particles::tests::plain_def()
        });
        child.def.timing = benilla_formats::EmitTiming::constant(100.0);
        // Point emitter, zero speed: births land on the parent with only the inherited velocity.
        let c_now = benilla_formats::ParamsNow {
            emission_speed: 0.0,
            area_length: 0.0,
            area_width: 0.0,
            ..crate::particles::emit::tests::now()
        };
        // Two live parents, far apart, distinct velocities.
        let parents = [
            particle(Vec3::X * 10.0, Vec3::Y * 3.0),
            particle(Vec3::X * -10.0, Vec3::Y * -3.0),
        ];
        // 100/s × 0.1 s × 2 calls = 20 births per frame.
        super::drive_child(
            &mut child,
            &c_now,
            &parents,
            100.0,
            true,
            1.0,
            0.1,
            true,
            &Transform::IDENTITY,
        );
        assert_eq!(child.particles.len(), 20, "one rate·dt per live parent");
        for p in &child.particles {
            let at_a = (p.pos - Vec3::X * 10.0).length() < 1e-3;
            let at_b = (p.pos + Vec3::X * 10.0).length() < 1e-3;
            assert!(at_a || at_b, "born at a parent particle");
            let v = if at_a { Vec3::Y * 3.0 } else { Vec3::Y * -3.0 };
            // speed 0 ⇒ the whole birth velocity is the inherited (1+S11·0)·parentVel.
            assert!(
                (p.vel - v).length() < 1e-3,
                "inherits its parent's velocity"
            );
        }
        // An empty parent pool drives nothing.
        let n = child.particles.len();
        super::drive_child(
            &mut child,
            &c_now,
            &[],
            100.0,
            true,
            1.0,
            0.1,
            true,
            &Transform::IDENTITY,
        );
        assert_eq!(child.particles.len(), n, "no parents, no births");
    }
}
