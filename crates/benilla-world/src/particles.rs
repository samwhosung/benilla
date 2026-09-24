//! M2 particle emitters, simulated on the CPU as the 1.12 client does: each [`ParticleEmitter`]
//! ages and integrates its pool every frame and expands it into camera-facing quads in the shared
//! effect stream ([`buffer::EffectQuads`]) that the effect lane ([`render`]) draws. File flag
//! `0x10` picks a particle's storage frame, and so whether a moving host leaves a trail; colour,
//! alpha, size and texture cell follow the [`benilla_formats::OverLife`] ramp by normalized age.

use benilla_assets::ModelEmitter;
use benilla_formats::ParticleEmitterDef;
use bevy::prelude::*;

pub mod buffer;
mod depthdump;
mod dumps;
pub(crate) mod emit;
mod emitdump;
mod model;
mod quads;
pub mod render;
pub(crate) mod sim; // `SceneGates` is the ribbon sim's draw-set input too

use emit::{emit_local, next_u32, rand01, rand_s11};
use sim::simulate_particles;
// The water-plane classification, shared with the ribbon sim.
pub(crate) use sim::{far_side_of_water, model_far_side, water_height, WaterInterleave};

/// A camera our own pacing (`boothHalfRate`) skips on some frames while its scene poses at full
/// rate. The reference's draw-set law (a tick only in a drawn frame, no catch-up) keys on its cull,
/// not this skip, so emitters keep simulating through it. `portrait::gate_booth_cameras` keeps it
/// on for as long as the camera is paced, so the archetype does not churn.
#[derive(Component)]
pub struct ViewThrottled;

/// A backstop cap on one emitter's live particles; a campfire's steady state is about 30 + 24.
const MAX_PARTICLES: usize = 1024;

/// Particle settings: the reference's `particleDensity` CVar, set from the debug panel.
#[derive(Resource)]
pub struct ParticleTuning {
    /// Clamped to [0.25, 1.0] by its handler (`0x688fb0`); it scales emission rate only (its
    /// getter's two callers are the spawn-count `fmul`s), never size, alpha or draw distance.
    pub(crate) density: f32,
}

impl Default for ParticleTuning {
    fn default() -> Self {
        Self { density: 1.0 }
    }
}

/// One live particle, in the frame file flag `0x10` picks. Clear (world mode): `pos`/`vel` are
/// world coordinates, Bevy axes (a transport's deck while riding one); the birth bakes bone pose,
/// rotation, scale and position through the live emitter matrix (`0x7b8acf`/`0x7b8b0f`) and the
/// draw folds nothing back (`0x7b3f48` omits `rt+0x1fc`), so a moving host leaves a trail. Set
/// (model mode): `pos`/`vel` are WoW model space, Z up, and the draw re-applies the live placement
/// (`0x7b8aa5`/`0x7b3efb`), the rigid ride of a carried torch.
struct Particle {
    pos: Vec3,
    vel: Vec3,
    age: f32,
    /// The emitter's lifespan channel sampled at birth (it animates: Frost Nova's runs 0.47 to
    /// 0.80 s). The particle dies at `age >= life`, and its over-life ramps normalize by it.
    life: f32,
    /// A random twinkle-table phase; the reference hashes the particle's pointer to the same end.
    phase: u32,
    /// `particle+0xd` bit 1: the first integrate skips the follow-delta add (`0x7b2680`).
    fresh: bool,
    /// Geometry particles only (`0x7b2420`, integrator `0x7b28e0`): stored-frame orientation and
    /// body-frame angular velocity; a quad particle carries identity and zero.
    quat: Quat,
    angvel: Vec3,
}

impl ParticleEmitter {
    /// The rig bone this emitter is mounted on, which the depth probes filter by.
    pub fn bone(&self) -> u16 {
        self.def.bone
    }
}

/// One spawned emitter: def, placement, live pool and emission state.
#[derive(Component)]
pub struct ParticleEmitter {
    def: ParticleEmitterDef,
    /// Model to world, Bevy space, refreshed from [`Self::owner`] when set.
    placement: Transform,
    /// World-mode particles' frame: world, or a ridden transport's deck ([`crate::ride_frame`]),
    /// as the reference divides the ride matrix out at birth (`rt+0x1fc = srcMx · A⁻¹`) and folds
    /// it back at draw (`0xcf5b68 = A · T · S`), so a rider's cloud does not trail the vehicle.
    pub(crate) ride: crate::ride_frame::StoredFrame,
    /// The entity followed, whose world transform becomes [`Self::placement`] each frame.
    owner: Option<Entity>,
    on_owner_loss: OwnerLoss,
    /// Draining after the owner's loss: no emission, the pool ages out in place, then despawns, as
    /// the reference's `HasLiveParticles` latch (`0x7b5f60`) keeps a disabled emitter ticking.
    draining: bool,
    /// The model instance whose composed alpha multiplies this cloud: the reference's
    /// `emitter+0x1a8`, a per-frame copy of the model's `+0x19c` (`0x718960` @`0x719073`) folded
    /// in by the over-life sampler (`0x7b9b10` @`0x7b9b42`). A doodad multiplies its distance fade.
    alpha_src: Option<Entity>,
    alpha: f32,
    /// The model (never the bone joint) whose live translation is this pool's sort point, and
    /// only that: a world-mode store is frozen at birth. `None` sorts at the spawn placement.
    anchor: Option<Entity>,
    /// The object's light node ([`crate::interior::ParticleLight`]), held so a lit child wired
    /// under an unlit parent can register its edge late ([`wire_child_emitters`]).
    light_node: Option<Entity>,
    /// The anchor's last known world translation, kept so a draining pool stays in place.
    anchor_pos: Vec3,
    particles: Vec<Particle>,
    accumulator: f32,
    /// The emitter origin's world position last frame, the reference's `rt+0x248` (`0x7b5230`
    /// @`0x7b5265`): the delta behind follow-delta and velocity inherit.
    emitter_prev: Option<Vec3>,
    /// Velocity inherit (file flag `0x40`, `0x7b5230`): the ~30 Hz trigger accumulator
    /// (`rt+0x254`) and the velocity held between triggers (`rt+0x258`).
    inherit_accum: f32,
    inherit_vel: Vec3,
    /// Last frame's `enabled && rate > 0`: a burst emitter (file flag `0x8000`) births its one
    /// `ftol(rate)` puff on the rising edge (the reference's `block+0x168`, `0x718f06`).
    gate_prev: bool,
    /// Seconds since spawn, the clip clock of a pinned lane's rate track.
    age: f32,
    /// The entity whose live `AnimationPlayer` picks the sequence and clip time the tracks sample.
    host: Option<Entity>,
    /// The sequence slot the timing samples: the idle seed, then the rig's or host's resolved one.
    seq: Option<usize>,
    rng: u32,
    /// The owner's reach in world yards, the draw-order rung ([`owner_last_bias`]) of this cloud
    /// and its children, which draw at the parent's anchor and so must clear the parent's owner.
    owner_reach: f32,
    /// The owner's model-local bound sphere, for the water-plane classification.
    water_bound: (Vec3, f32),
    /// No quads until it is resident, or the engine fallback flashes through the additive blend.
    texture: Handle<Image>,
    /// True while the owner is out of the frame's draw set; a gated pool pushes no quads.
    gated: bool,
    /// The owner's freeze, for a scene its camera cannot report (all `<Model>` tile panes share one
    /// camera): it holds pool and age, but never on a [`Self::draining`] emitter.
    frozen: bool,
    /// The pending recursion model (`0x7b5dd0`); [`wire_child_emitters`] turns up to 4 of its
    /// emitters (`0x7b5dfe`) into [`Self::children`].
    recursion: Option<Handle<benilla_assets::M2Model>>,
    /// Child emitters, driven once per live parent particle per frame, never ambiently.
    children: Vec<ChildEmitter>,
    /// The geometry model (`0x7b1c80`, spawn driver `0x7b5550`): when authored, particles draw as
    /// its instances instead of quads ([`model::update_model_particles`]).
    geometry: Option<Handle<benilla_assets::M2Model>>,
    /// One instance per drawn particle, grown on demand and hidden past the live count.
    model_instances: Vec<model::ModelInstance>,
    /// The half-extent unit ([`quads::DrawFrame::size_scale`]): a yard, or a UI tile's pixels.
    size_scale: f32,
    /// The render-target rectangle, in pixels, this cloud is confined to ([`Self::set_clip`]).
    pub(crate) clip: Option<Vec4>,
}

/// A child emitter: the recursion model's def, texture and pool, drawn at the parent's anchor.
struct ChildEmitter {
    def: ParticleEmitterDef,
    texture: Handle<Image>,
    particles: Vec<Particle>,
    accumulator: f32,
    gate_prev: bool,
    rng: u32,
}

/// Marks a geometry-particle instance ([`model`]), keeping the sim's reads and writes disjoint.
#[derive(Component)]
pub struct ChildDraw;

#[cfg(test)]
impl ChildEmitter {
    fn bare(def: ParticleEmitterDef) -> Self {
        Self {
            def,
            texture: Handle::default(),
            particles: Vec::new(),
            accumulator: 0.0,
            gate_prev: false,
            rng: 7,
        }
    }
}

/// What a live pool does when its owner entity goes away, which only the spawn site knows.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum OwnerLoss {
    /// The owner model was destroyed (an item replaced, a unit streamed out): free the pool with
    /// it, as the reference frees a model's emitters at its dtor (`0x70e313`).
    Free,
    /// The effect ended (a missile impacted): stop emitting and let the pool age out in place, as
    /// the reference's model outlives its draining emitters.
    #[default]
    Drain,
}

/// The live entities an emitter rides, named so a call site cannot swap them.
#[derive(Clone, Copy, Default)]
pub struct EmitterFrames {
    /// The entity whose live transform is the placement: `(entity, [0; 3])` for a whole model,
    /// `(joint, bone_pivot)` on an animated bone; `None` for a static doodad.
    pub owner: Option<(Entity, [f32; 3])>,
    /// The cloud anchor: the model, never the bone ([`ParticleEmitter::anchor`]).
    pub anchor: Option<Entity>,
    /// The model instance whose render alpha multiplies these particles.
    pub alpha: Option<Entity>,
    /// The light node: a creature's root, an equipped item's wearer (the reference aliases the
    /// wearer's collector into each attached model); `None` for a doodad, a WMO prop or a booth.
    pub light_node: Option<Entity>,
    /// What losing `owner` means ([`OwnerLoss`]).
    pub on_owner_loss: OwnerLoss,
}

impl ParticleEmitter {
    /// Live particle count, for the perf probe ([`crate::capture`]).
    pub fn live(&self) -> usize {
        self.particles.len()
    }

    /// Set the half-extent unit; a UI model tile passes the reference's pixels per model unit.
    pub fn set_size_scale(&mut self, size_scale: f32) {
        self.size_scale = size_scale;
    }

    /// Confine this cloud's quads to a UI tile's cell of the shared atlas, in pixels
    /// ([`crate::particles::buffer::EffectDrawSpec::clip`]), as the reference draws each `<Model>`
    /// with the widget's rect as its viewport; `None` draws over the whole target.
    pub fn set_clip(&mut self, clip: Option<Vec4>) {
        if self.clip != clip {
            self.clip = clip;
        }
    }

    /// Freeze or thaw this cloud from the owner's side ([`Self::frozen`]). Thawing clears
    /// [`Self::gated`]: the draw-set early-out reads none of the owner's inputs, so a cloud left
    /// gated on a still camera would never re-enter its draw set.
    pub fn set_frozen(&mut self, frozen: bool) {
        self.frozen = frozen;
        if !frozen {
            self.gated = false;
        }
    }

    /// Whether the owner froze this cloud, so an owner can skip a same-value write.
    pub fn is_frozen(&self) -> bool {
        self.frozen
    }

    /// The authored def, for the particle census probe ([`crate::capture`]).
    pub fn def(&self) -> &ParticleEmitterDef {
        &self.def
    }

    /// The particle texture, for the phase probe ([`crate::capture`]).
    pub fn texture(&self) -> &Handle<Image> {
        &self.texture
    }

    /// The owner-last rung ([`owner_last_bias`]), which geometry particles share too: the
    /// reference draws a model's emitters in one bracket.
    pub(super) fn owner_rung(&self) -> f32 {
        owner_last_bias(self.owner_reach)
    }

    /// Whether the draw-set gate let this emitter tick and push quads this frame.
    pub fn drawn(&self) -> bool {
        !self.gated
    }

    /// This frame's model render alpha, for the geometry-particle lane's `MeshTag`.
    pub fn render_alpha(&self) -> f32 {
        self.alpha
    }

    /// The model instance this emitter belongs to, by which an effect lane finds its emitters.
    pub fn anchor(&self) -> Option<Entity> {
        self.anchor
    }

    /// The emission clock's host ([`EmitClock::Host`]); `None` for a pinned or effect clock.
    pub fn emit_host(&self) -> Option<Entity> {
        self.host
    }

    /// Switch to [`OwnerLoss::Drain`]: the effect is ending, so its particles finish rather than
    /// pop. The reference's `CEffect` teardown (`0x6203e0`) only hides and queues the node, which
    /// stops emission; the draw admits by live-particle count (`emitter+0x64`, `0x7b4b46`), so the
    /// survivors age out, and `0x61f680` frees the node once `CM2Model+0x3d8` reads zero. An
    /// attached instance spawns `Free`, since a model dtor frees its emitters at once.
    pub fn drain_on_owner_loss(&mut self) {
        self.on_owner_loss = OwnerLoss::Drain;
    }

    /// The cloud's live world anchor, the census probe's distance subject.
    pub fn anchor_world(&self) -> Vec3 {
        self.anchor_pos
    }

    /// The cloud's world centroid, plane normal, RMS thickness and RMS in-plane radius as the
    /// quads draw it, the census probe's orientation measure; `None` under 4 particles.
    pub fn cloud_fingerprint(&self) -> Option<(Vec3, Vec3, f32, f32)> {
        if self.particles.len() < 4 {
            return None;
        }
        let anchored = !self.def.model_space();
        let world: Vec<Vec3> = self
            .particles
            .iter()
            .map(|p| {
                if anchored {
                    self.ride.to_world(p.pos)
                } else {
                    self.placement
                        .transform_point(benilla_assets::coords::wow_to_bevy([
                            p.pos.x, p.pos.y, p.pos.z,
                        ]))
                }
            })
            .collect();
        let n = world.len() as f32;
        let centroid = world.iter().sum::<Vec3>() / n;
        // The normal crosses the covariance's two largest axes, found by power iteration.
        let mut cov = Mat3::ZERO;
        for w in &world {
            let d = *w - centroid;
            cov += Mat3::from_cols(d * d.x, d * d.y, d * d.z);
        }
        let power = |m: &Mat3, seed: Vec3| {
            let mut v = seed;
            for _ in 0..32 {
                let next = *m * v;
                if next.length_squared() < 1e-12 {
                    return seed.normalize_or(Vec3::X);
                }
                v = next.normalize();
            }
            v
        };
        let v1 = power(&cov, Vec3::new(0.7, 0.5, 0.5));
        // Deflate v1, then find the second axis in the remaining plane.
        let l1 = (cov * v1).dot(v1);
        let deflated = cov - Mat3::from_cols(v1 * v1.x, v1 * v1.y, v1 * v1.z) * l1;
        let v2 = power(&deflated, v1.any_orthonormal_vector());
        let normal = v1.cross(v2).normalize_or(Vec3::Y);
        let (mut thick2, mut radial2) = (0.0f32, 0.0f32);
        for w in &world {
            let d = *w - centroid;
            let t = d.dot(normal);
            thick2 += t * t;
            radial2 += d.length_squared() - t * t;
        }
        Some((centroid, normal, (thick2 / n).sqrt(), (radial2 / n).sqrt()))
    }
}

/// A placed model's draw-set gate, shared with its ribbons ([`crate::ribbons`]): out of the draw
/// set an emitter neither simulates nor draws, as the reference ticks particles in the owning
/// model's animate step, run only for drawn models; inside it only emission LOD applies.
/// Entity-owned emitters carry none: server visibility bounds them.
#[derive(Component, Clone)]
pub struct EmitterFade {
    /// The owner doodad's world bounding-sphere radius, which picks the fade band.
    pub radius: f32,
    /// The owner's world bbox centre, which the fade measures to, not the emitter's position.
    pub center: Vec3,
    /// The WMO placement whose doodad set holds the owner (`None` for an ADT doodad), for the
    /// exterior-window exemption: a prop of the building the camera stands in keeps burning.
    pub(crate) instance: Option<Entity>,
    /// The rooms of [`Self::instance`] naming the owner, built with it in `terrain_stream::spawn`:
    /// the reference admits a WMO prop to the frame's scene only for the groups its portal walk
    /// reaches (`0x685d70` → `0x6838f0`), so a prop in a culled room has no emitters to tick. By
    /// value, since a `WmoGroupVis` component would enlist the emitter in `apply_model_visibility`.
    pub(crate) room: Option<crate::wmo_portal::WmoGroupVis>,
}

impl EmitterFade {
    /// The gate of a placement that is nobody's furniture, `center` in world space; a building's
    /// prop goes through [`crate::terrain_stream::emitter_fade`].
    pub(crate) fn sphere(radius: f32, center: Vec3) -> Self {
        Self {
            radius,
            center,
            instance: None,
            room: None,
        }
    }

    /// The owner's distance-fade alpha, which multiplies its particles as it does its batches
    /// (`0x683f80` → `+0x180` → `+0x19c` → `emitter+0x1a8`); the draw set ends at its zero.
    pub fn distance_alpha(&self, cam_pos: Vec3) -> f32 {
        let (dx, dz) = (self.center.x - cam_pos.x, self.center.z - cam_pos.z);
        crate::model_fade::doodad_fade_alpha(self.radius, (dx * dx + dz * dz).sqrt())
    }

    /// Whether the owner is in the frame's scene worklist, so its emitters tick and draw. The
    /// first four terms test the owner's fade sphere (`[rec+0x68]`):
    /// 1. the size-banded distance fade is above zero (`0x683f80`),
    /// 2. the sphere is inside the far-clip wall ([`crate::view::within_farclip`]), since term 1
    ///    admits a never-fading owner (a brazier) at any distance,
    /// 3. `lateral_in_frustum`, the side and near planes,
    /// 4. `exterior_admitted`: the reference links a doodad into the worklist through the
    ///    per-window populate walk (`0x683700`), and an unlinked one emits nothing,
    /// 5. `room_admitted`, the portal PVS of the owner's room ([`Self::room_admitted`]).
    pub fn in_draw_set(
        &self,
        cam_pos: Vec3,
        cam_fwd: Vec3,
        farclip: f32,
        lateral_in_frustum: bool,
        exterior_admitted: bool,
        room_admitted: bool,
    ) -> bool {
        let (dx, dz) = (self.center.x - cam_pos.x, self.center.z - cam_pos.z);
        let horiz = (dx * dx + dz * dz).sqrt();
        crate::model_fade::doodad_fade_alpha(self.radius, horiz) > 0.0
            && crate::view::within_farclip(farclip, cam_pos, cam_fwd, self.center, self.radius)
            && lateral_in_frustum
            && exterior_admitted
            && room_admitted
    }

    /// Term 5 ([`crate::wmo_portal::room_admits`], failing closed when the placement is gone).
    pub fn room_admitted(&self, instance: Option<&crate::wmo_portal::WmoPortalInstance>) -> bool {
        crate::wmo_portal::room_admits(self.room.as_ref(), instance)
    }

    /// Term 4, given the frame's gate and the placement the camera is inside.
    pub fn exterior_admitted(
        &self,
        gate: &crate::exterior_cull::ExteriorGate,
        camera_instance: Option<Entity>,
    ) -> bool {
        if self.instance.is_some() && self.instance == camera_instance {
            return true; // a prop of the building the camera stands in is not exterior to it
        }
        gate.admits_sphere(self.center, self.radius)
    }
}

/// Spawn an emitter for one [`ModelEmitter`] at `placement`; `None` when it would draw nothing.
pub fn spawn_emitter(
    commands: &mut Commands,
    emitter: &ModelEmitter,
    placement: Transform,
    frames: EmitterFrames,
    clock: EmitClock,
) -> Option<Entity> {
    // Perf-bisect kill-switch: $WOW_NO_PARTICLES spawns no emitters at all.
    if std::env::var_os("WOW_NO_PARTICLES").is_some() {
        return None;
    }
    // No texture: a quad emitter draws nothing, a geometry one keeps a never-resident default.
    let texture = match emitter.texture.clone() {
        Some(t) => t,
        None if emitter.geometry.is_some() => Handle::default(),
        None => return None,
    };
    let mut def = emitter.def.clone();
    let owner = frames.owner.map(|(entity, pivot)| {
        // Rebase the emitter origin into the owner's frame, bone-local for a joint (raw WoW axes).
        def.position = [
            def.position[0] - pivot[0],
            def.position[1] - pivot[1],
            def.position[2] - pivot[2],
        ];
        entity
    });
    // The rate's peak over every sequence, not its first key: a one-shot burst keys `0 → 200 → 0`.
    if def.params.peak_lifespan() <= 0.0 || def.timing.peak_rate() <= 0.0 {
        return None; // emits nothing
    }
    // The model's loader-idle sequence, which the reference arms on every M2 at load; slot 0 can
    // be a Spawn flourish (a Spawn/Stand/Despawn GameObject).
    let idle = Some(emitter.idle_seq);
    let (host, seq) = match clock {
        EmitClock::Pinned => (None, idle),
        EmitClock::Effect(s) => (None, s.or(idle)),
        EmitClock::Host(h) => (Some(h), idle),
    };
    // The reach is model-local and the rung a view-space distance, so it takes the scale.
    let owner_reach = emitter.owner_reach * placement.scale.max_element();
    // Seed the RNG from the placement position so two campfires don't flicker in lockstep.
    let t = placement.translation;
    let rng = (t.x.to_bits() ^ t.y.to_bits().rotate_left(11) ^ t.z.to_bits().rotate_left(22))
        .wrapping_mul(0x9E37_79B9)
        | 1;
    // A lit emitter consumes its object's light node as a lit batch does, and is often the only
    // consumer (the Onyxia lava trap's batches are all unlit), so it registers the edge itself.
    let lit_node = frames.light_node.filter(|_| def.lit);
    let mut spawned = commands.spawn((
        // The sim writes the anchor here for the probes; the draw's sort key rides its record.
        Transform::IDENTITY,
        ParticleEmitter {
            def,
            placement,
            ride: crate::ride_frame::StoredFrame::default(),
            owner,
            on_owner_loss: frames.on_owner_loss,
            draining: false,
            alpha_src: frames.alpha,
            alpha: 1.0,
            anchor: frames.anchor,
            light_node: frames.light_node,
            anchor_pos: placement.translation,
            particles: Vec::new(),
            accumulator: 0.0,
            emitter_prev: None,
            inherit_accum: 0.0,
            inherit_vel: Vec3::ZERO,
            gate_prev: false,
            age: 0.0,
            host,
            seq,
            rng,
            owner_reach,
            water_bound: emitter.water_bound,
            texture,
            gated: false,
            frozen: false,
            recursion: emitter.recursion.clone(),
            children: Vec::new(),
            geometry: emitter.geometry.clone(),
            model_instances: Vec::new(),
            size_scale: 1.0,
            clip: None,
        },
    ));
    if emitter.recursion.is_some() {
        spawned.insert(PendingChildren);
    }
    if let Some(node) = lit_node {
        spawned.insert(crate::interior::EmitterLitBy(node));
    }
    Some(spawned.id())
}

/// The depth bias that draws a model's effects after its own transparent batches, as the reference
/// brackets a model's emitters after its batches (`0x70d8b0`). Sort keys are view z plus bias,
/// ascending ([`crate::sky_order`]); every batch centre lies within `reach` world yards of the
/// origin and view z is 1-Lipschitz, so a rung above `reach` clears them. All of a model's effects
/// take one rung, as the reference draws them in file order within the one bracket;
/// [`benilla_formats::owner_last_rung`] rounds it.
pub(crate) fn owner_last_bias(reach: f32) -> f32 {
    benilla_formats::owner_last_rung(reach)
}

/// An emitter whose recursion model has not resolved, the only kind [`wire_child_emitters`] visits.
#[derive(Component)]
pub(crate) struct PendingChildren;

/// Once a parent's recursion model resolves, make up to 4 of its emitters (`0x7b5dfe`) the
/// parent's children, as the reference does at the model's async-load completion (`0x7b5dd0`).
pub(crate) fn wire_child_emitters(
    mut commands: Commands,
    models: Res<Assets<benilla_assets::M2Model>>,
    mut emitters: Query<
        (
            Entity,
            &mut ParticleEmitter,
            Has<crate::interior::EmitterLitBy>,
        ),
        With<PendingChildren>,
    >,
) {
    for (entity, mut emitter, registered) in &mut emitters {
        let Some(model) = emitter.recursion.as_ref().and_then(|h| models.get(h)) else {
            continue;
        };
        let mut rng_seed = emitter.rng.rotate_left(7) | 1;
        let children: Vec<ChildEmitter> = model
            .emitters
            .iter()
            .take(4)
            .filter_map(|em| {
                let texture = em.texture.clone()?;
                if em.def.params.peak_lifespan() <= 0.0 || em.def.timing.peak_rate() <= 0.0 {
                    return None;
                }
                Some(ChildEmitter {
                    def: em.def.clone(),
                    texture,
                    particles: Vec::new(),
                    accumulator: 0.0,
                    gate_prev: false,
                    rng: {
                        rng_seed = rng_seed.wrapping_mul(0x9E37_79B9) | 1;
                        rng_seed
                    },
                })
            })
            .collect();
        emitter.recursion = None;
        emitter.children = children;
        commands.entity(entity).remove::<PendingChildren>();
        // A child has its own flag word and so its own lighting verdict: a lit child under an
        // unlit parent registers the light edge here, since nothing did at spawn.
        if !registered && emitter.children.iter().any(|c| c.def.lit) {
            if let Some(node) = emitter.light_node {
                commands
                    .entity(entity)
                    .insert(crate::interior::EmitterLitBy(node));
            }
        }
    }
}

/// The sequence clock the rate and enabled tracks sample ([`benilla_formats::EmitTiming`]).
#[derive(Clone, Copy, Default)]
pub enum EmitClock {
    /// The loader-idle slot on the spawn-age clock: a placed doodad, a booth, a lane with no rig.
    #[default]
    Pinned,
    /// A spell effect's armed slot (`None` is the idle) on the spawn-age clock: the instance is
    /// fresh per play, so its global-sequence loops open at phase 0 with it.
    Effect(Option<usize>),
    /// A live `AnimationPlayer` picks the slot and clip time each frame, as the reference's animate
    /// kernel samples the current sequence (`0x714260`).
    Host(Entity),
}

/// Load `accumulator` with this frame's owed births (the reference's emitter pass and spawn
/// driver, `0x718960`/`0x7b5550`), gated on `emitting && rate > 0`. A continuous emitter (file flag
/// `0x8000` clear) pours `rate · scale · dt`; a burst loads `ftol(rate · scale)` once on the gate's
/// rising edge and re-arms when it falls. Returns the burst, for the `fx` trace.
fn accumulate_emission(
    is_burst: bool,
    rate: f32,
    emitting: bool,
    scale: f32,
    dt: f32,
    accumulator: &mut f32,
    gate_prev: &mut bool,
) -> f32 {
    if !emitting {
        *accumulator = 0.0;
    }
    let rate = if emitting { rate.max(0.0) } else { 0.0 };
    let gate = rate > 0.0;
    let mut burst = 0.0;
    if is_burst {
        if gate && !*gate_prev {
            burst = (rate * scale).trunc();
            *accumulator = burst;
        }
    } else if gate {
        *accumulator += rate * scale * dt;
    }
    *gate_prev = gate;
    burst
}

/// Registers the effect lane and the particle systems; emitters come from [`spawn_emitter`].
pub struct ParticlePlugin;

impl Plugin for ParticlePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(render::EffectLanePlugin)
            .init_resource::<ParticleTuning>()
            .init_resource::<buffer::EffectQuads>()
            // After the billboard joint palette: an emitter on a billboarded bone must read this
            // frame's replaced pose, which an `Update` read loses (avian's fixed-loop sync
            // re-propagates from locals). `begin_effect_frame` clears the stream before both
            // writers, this sim and the ribbon sim, so their order is free.
            .add_systems(
                PostUpdate,
                (
                    buffer::begin_effect_frame
                        .before(simulate_particles)
                        .before(crate::ribbons::simulate_ribbons),
                    wire_child_emitters,
                    simulate_particles,
                    model::update_model_particles,
                )
                    .chain()
                    .in_set(crate::billboard::BillboardPlace)
                    .after(crate::billboard::billboard_joint_palette)
                    .after(crate::rig_anim::finalize_rig_worlds)
                    // And after card facing: an item's emitter can ride a meshless billboard card.
                    .after(crate::billboard::face_billboards),
            )
            // Re-arm the write-order tripwire (`buffer::EffectQuads::cleared_this_frame`).
            .add_systems(Last, buffer::clear_effect_frame_flag);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use benilla_formats::ParticleShape;

    /// The minimal def, for sibling-module tests.
    pub(crate) fn plain_def() -> ParticleEmitterDef {
        super::emit::tests::def(ParticleShape::Plane)
    }

    fn model_emitter(idle_seq: usize) -> ModelEmitter {
        ModelEmitter {
            def: plain_def(),
            texture: Some(Handle::default()),
            bone_pivot: [0.0; 3],
            billboard: None,
            recursion: None,
            geometry: None,
            owner_reach: 0.0,
            water_bound: (Vec3::ZERO, 0.0),
            idle_seq,
        }
    }

    fn seeded_slot(emitter: ModelEmitter, clock: EmitClock) -> Option<usize> {
        use bevy::ecs::system::RunSystemOnce;
        let mut app = App::new();
        let e = app
            .world_mut()
            .run_system_once(move |mut c: Commands| {
                spawn_emitter(&mut c, &emitter, Transform::IDENTITY, default(), clock)
            })
            .unwrap()?;
        app.world().get::<ParticleEmitter>(e).unwrap().seq
    }

    /// A Spawn/Stand/Despawn GameObject (the battlefield banners) keeps its idle in slot 1, slot 0
    /// being the Spawn flourish. Every clock takes the seed: `Host` because its player may arm
    /// nothing, `Effect(None)` because an unarmed rig rests on the idle.
    #[test]
    fn an_emitter_opens_on_its_models_idle_slot_not_slot_zero() {
        let host = Entity::from_raw_u32(1).unwrap();
        for clock in [
            EmitClock::Pinned,
            EmitClock::Effect(None),
            EmitClock::Host(host),
        ] {
            assert_eq!(seeded_slot(model_emitter(1), clock), Some(1));
        }
        // An armed effect slot still wins: the seed is a default.
        assert_eq!(
            seeded_slot(model_emitter(1), EmitClock::Effect(Some(2))),
            Some(2)
        );
        assert_eq!(seeded_slot(model_emitter(0), EmitClock::Pinned), Some(0));
    }

    /// An attached instance spawns `Free`, so a model dtor takes its pool; an ending effect flips
    /// it to `Drain` (`0x6203e0` hides the node, `0x61f680` frees it once drained).
    #[test]
    fn an_ending_instance_switches_its_emitters_from_free_to_drain() {
        use bevy::ecs::system::RunSystemOnce;
        let root = Entity::from_raw_u32(7).unwrap();
        let mut app = App::new();
        let e = app
            .world_mut()
            .run_system_once(move |mut c: Commands| {
                spawn_emitter(
                    &mut c,
                    &model_emitter(0),
                    Transform::IDENTITY,
                    EmitterFrames {
                        anchor: Some(root),
                        on_owner_loss: OwnerLoss::Free,
                        ..default()
                    },
                    EmitClock::Pinned,
                )
            })
            .unwrap()
            .expect("the emitter spawns");

        assert_eq!(
            app.world().get::<ParticleEmitter>(e).unwrap().anchor(),
            Some(root),
            "the instance root is the identity its lane finds this emitter by",
        );
        assert_eq!(
            app.world().get::<ParticleEmitter>(e).unwrap().on_owner_loss,
            OwnerLoss::Free,
            "spawned Free — a torn-down model must not strand its cloud",
        );

        app.world_mut()
            .get_mut::<ParticleEmitter>(e)
            .unwrap()
            .drain_on_owner_loss();
        assert_eq!(
            app.world().get::<ParticleEmitter>(e).unwrap().on_owner_loss,
            OwnerLoss::Drain,
            "the ending instance's particles finish instead of popping",
        );
    }

    /// The camera at the origin looking down −Z, the owner sphere `depth` yd straight ahead.
    fn gate(radius: f32, depth: f32, farclip: f32) -> bool {
        EmitterFade {
            radius,
            center: Vec3::new(0.0, 0.0, -depth),
            instance: None,
            room: None,
        }
        .in_draw_set(Vec3::ZERO, Vec3::NEG_Z, farclip, true, true, true)
    }

    /// The rung must clear the reach strictly, or a batch at the model's edge ties with the effect;
    /// an integer reach would tie under a bare `ceil`. 3.666 yd is a voidwalker's vertex bound.
    #[test]
    fn the_owner_last_rung_always_clears_the_owner() {
        for reach in [0.0f32, 0.5, 1.0, 3.666, 4.0, 7.999, 12.0, 31.5] {
            let rung = owner_last_bias(reach);
            assert!(rung > reach, "rung {rung} must clear reach {reach}");
            assert_eq!(rung, rung.trunc(), "rung {rung} must be a whole yard");
        }
        // Capped below `sky_order`'s rungs: a huge model loses its own ordering, not the world's.
        assert_eq!(owner_last_bias(500.0), 32.0);
        assert_eq!(owner_last_bias(-1.0), 1.0);
    }

    #[test]
    fn a_never_fading_owner_is_still_bounded_by_the_wall() {
        let big = crate::model_fade::NEVER_FADE_RADIUS + 5.0;
        assert_eq!(crate::model_fade::doodad_fade_alpha(big, 5000.0), 1.0);
        assert!(gate(big, 500.0, 777.0), "inside the wall: draws");
        assert!(!gate(big, 1000.0, 777.0), "past the wall: must NOT draw");
        // The live farclip, not a constant: out at the minimum view distance, in at the maximum.
        assert!(!gate(big, 500.0, 177.0));
        assert!(gate(big, 1000.0, 1200.0));
    }

    #[test]
    fn the_wall_does_not_replace_the_size_fade() {
        assert!(gate(0.3, 45.0, 777.0), "mid-band: still drawing");
        assert!(
            !gate(0.3, 60.0, 777.0),
            "past the 50-yd band end, nowhere near the wall"
        );
    }

    #[test]
    fn the_lateral_frustum_term_is_anded() {
        let f = EmitterFade {
            radius: 2.0,
            center: Vec3::new(0.0, 0.0, -60.0),
            instance: None,
            room: None,
        };
        assert!(f.in_draw_set(Vec3::ZERO, Vec3::NEG_Z, 777.0, true, true, true));
        assert!(!f.in_draw_set(Vec3::ZERO, Vec3::NEG_Z, 777.0, false, true, true));
        // So is the exterior-window term.
        assert!(!f.in_draw_set(Vec3::ZERO, Vec3::NEG_Z, 777.0, true, false, true));
        // And the room term: the window admits the building, the PVS the room.
        assert!(!f.in_draw_set(Vec3::ZERO, Vec3::NEG_Z, 777.0, true, true, false));
    }

    /// A building's props are not exterior to a camera inside it; an ADT doodad belongs to none.
    #[test]
    fn a_sealed_room_keeps_its_own_props_burning_and_stops_everything_else() {
        use crate::exterior_cull::ExteriorGate;
        let mut w = World::new();
        let (here, elsewhere) = (w.spawn_empty().id(), w.spawn_empty().id());
        let sealed = ExteriorGate::Windows(Vec::new());
        let fade = |instance| EmitterFade {
            radius: 2.0,
            center: Vec3::new(0.0, 0.0, -60.0),
            instance,
            room: None,
        };

        assert!(
            fade(Some(here)).exterior_admitted(&sealed, Some(here)),
            "a prop of the building the camera is in must keep emitting"
        );
        assert!(
            !fade(Some(elsewhere)).exterior_admitted(&sealed, Some(here)),
            "another building's prop is exterior scene, and the room is sealed"
        );
        assert!(
            !fade(None).exterior_admitted(&sealed, Some(here)),
            "an ADT map doodad belongs to no building — the reported mushroom"
        );
        // Outdoors the gate stands down.
        for instance in [Some(here), Some(elsewhere), None] {
            assert!(fade(instance).exterior_admitted(&ExteriorGate::Open, Some(here)));
        }
        // With no instance on either side, `None == None` is not the camera's own building.
        assert!(!fade(None).exterior_admitted(&sealed, None));
    }

    /// All four arms of the room gate, including the one that fails closed.
    #[test]
    fn a_prop_in_a_culled_room_emits_nothing_and_an_orphan_refuses() {
        use crate::wmo_portal::{WmoGroupVis, WmoPortalInstance};
        let cot = WmoPortalInstance {
            handle: Handle::default(),
            world_from_local: bevy::math::Affine3A::IDENTITY,
            name_set: 0,
            liquid_visited: vec![false; 3],
            flooded: vec![None; 3],
            visible: vec![true, false, false],
            interior_fog: vec![false; 3],
        };
        let fade = |groups: Option<&[u16]>| EmitterFade {
            radius: 2.0,
            center: Vec3::new(0.0, 0.0, -60.0),
            instance: None,
            room: groups.map(|g| WmoGroupVis {
                instance: Entity::PLACEHOLDER,
                groups: g.into(),
            }),
        };

        // Not a building's prop: never gated.
        assert!(
            fade(None).room_admitted(Some(&cot)),
            "an unclaimed emitter is never gated"
        );
        assert!(fade(Some(&[0])).room_admitted(Some(&cot)));
        assert!(!fade(Some(&[1, 2])).room_admitted(Some(&cot)));
        // A prop several rooms name draws while any is visible, the submeshes' `drawn_by` law.
        assert!(fade(Some(&[1, 0])).room_admitted(Some(&cot)));
        // Unlike the rest of the cull, a despawned placement fails closed.
        assert!(
            !fade(Some(&[0])).room_admitted(None),
            "an orphaned emitter draws nothing"
        );
    }

    /// Owners above `NEVER_FADE_RADIUS` only: there the fade term is a constant 1, so the gate is
    /// the wall alone; below it the mesh keeps drawing while the fade feathers it out.
    #[test]
    fn the_emitter_gate_agrees_with_the_owner_meshs_cull() {
        let (cam, fwd) = (Vec3::ZERO, Vec3::NEG_Z);
        for farclip in [177.0f32, 777.0, 1200.0] {
            for depth in [100.0f32, 700.0, 776.0, 777.0, 778.0, 900.0, 2000.0] {
                for radius in [crate::model_fade::NEVER_FADE_RADIUS + 0.01, 12.0, 40.0] {
                    let center = Vec3::new(0.0, 0.0, -depth);
                    assert_eq!(
                        crate::model_fade::doodad_fade_alpha(radius, depth),
                        1.0,
                        "precondition: this owner never size-fades"
                    );
                    let mesh_drawn = crate::view::within_farclip(farclip, cam, fwd, center, radius);
                    let emitter_drawn = EmitterFade {
                        radius,
                        center,
                        instance: None,
                        room: None,
                    }
                    .in_draw_set(cam, fwd, farclip, true, true, true);
                    assert_eq!(
                        mesh_drawn, emitter_drawn,
                        "wall disagreement at farclip {farclip} depth {depth} radius {radius}"
                    );
                }
            }
        }
    }

    /// File flag `0x8000`, as on the Feint and Eviscerate impact puffs.
    #[test]
    fn burst_emitter_fires_once_on_the_rising_edge() {
        let (mut acc, mut prev) = (0.0, false);
        assert_eq!(
            accumulate_emission(true, 0.0, true, 1.0, 0.016, &mut acc, &mut prev),
            0.0,
            "rate 0 — no gate, no burst"
        );
        assert_eq!(acc, 0.0);
        assert_eq!(
            accumulate_emission(true, 30.0, true, 1.0, 0.016, &mut acc, &mut prev),
            30.0,
            "the frame the rate rises: one full-count burst"
        );
        assert_eq!(acc, 30.0);
        acc = 0.0; // the birth loop drains it
        accumulate_emission(true, 30.0, true, 1.0, 0.016, &mut acc, &mut prev);
        accumulate_emission(true, 30.0, true, 1.0, 0.016, &mut acc, &mut prev);
        assert_eq!(acc, 0.0, "held-high gate stays latched — never a pour");
        // The gate falls and re-arms; the next rise bursts again, truncated through the scale.
        accumulate_emission(true, 30.0, false, 1.0, 0.016, &mut acc, &mut prev);
        assert_eq!(
            accumulate_emission(true, 30.0, true, 0.55, 0.016, &mut acc, &mut prev),
            16.0,
            "ftol(30 · 0.55) = 16"
        );
    }

    /// A continuous emitter drops its owed fraction while disabled.
    #[test]
    fn continuous_emitter_pours_rate_dt() {
        let (mut acc, mut prev) = (0.0, false);
        accumulate_emission(false, 30.0, true, 1.0, 0.1, &mut acc, &mut prev);
        accumulate_emission(false, 30.0, true, 1.0, 0.1, &mut acc, &mut prev);
        assert!((acc - 6.0).abs() < 1e-4, "30/s × 0.2 s, no burst latch");
        accumulate_emission(false, 30.0, false, 1.0, 0.1, &mut acc, &mut prev);
        assert_eq!(acc, 0.0, "disabled zeroes the owed fraction");
    }

    /// The follow-delta response (`0x7b5d30`): the line through the two authored (speed,
    /// fraction) samples, zeroed for equal speeds as in the reference.
    #[test]
    fn follow_line_matches_the_authored_two_point_response() {
        let mut d = super::emit::tests::def(ParticleShape::Plane);
        d.follow_speed1 = 1.0;
        d.follow_scale1 = 0.2;
        d.follow_speed2 = 3.0;
        d.follow_scale2 = 0.8;
        let (slope, intercept) = d.follow_line().expect("distinct speeds");
        assert!((slope * 2.0 + intercept - 0.5).abs() < 1e-6, "midpoint");
        assert!((slope * 1.0 + intercept - 0.2).abs() < 1e-6, "sample 1");
        d.follow_speed2 = 1.0;
        assert!(
            d.follow_line().is_none(),
            "equal speeds → the reference zeroes the response"
        );
    }
}
