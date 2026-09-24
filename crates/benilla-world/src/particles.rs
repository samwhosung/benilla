//! Faithful M2 particle emitters — the visible fire/glow of campfires, torches, braziers and candles.
//!
//! The 1.12 client simulates particles on the CPU and draws them as camera-facing, additive quads; we
//! do the same (the GPU never touched the legacy particle path, and a CPU sim is the only way to match
//! the integrator exactly — see decision 0014). Each [`ParticleEmitter`] owns a small pool of live
//! particles: every frame we age + integrate the pool, then expand each live particle into a
//! camera-facing quad written into the **shared effect-quad stream** ([`buffer::EffectQuads`],
//! decision 0732 slice P1 — one CPU vertex vec + one GPU upload for the whole family, drawn by
//! the dedicated lane in [`render`]; per-emitter `Mesh`/material assets are gone, and with them
//! the per-frame allocator churn they priced). Every cloud is **anchored to its MODEL's live
//! position** (the reference rebuilds the
//! draw matrix with `translate(−emitterPos)` each frame — a moving model carries its flame, no
//! world-frozen trails) while the emitter's BONE composes only each particle's birth: bone motion
//! is baked per particle (an emitter orbiting on an animated bone — the food sparkle's global-
//! sequence spin — births a moving ring of straight risers, never a swirling cloud; the client's
//! only live-follow plumbing is file flag `0x4000`, unauthored by our content). File flag `0x10`
//! additionally folds the live bone ROTATION at render instead of baking it at birth
//! (`0x7b3efb`). The lane's
//! fragment does the vanilla gamma-space combine (`shaders/wow_effect.wgsl`, decisions
//! 0152/0161). The whole effect family — particles, ribbons, the decal family, water foam,
//! precipitation — draws through the lane; `WowParticleMaterial` retired with slice P2 (0733).
//!
//! Over-life colour (incl. the alpha that is the additive weight), size and texture-cell are sampled
//! from the parsed [`benilla_formats::OverLife`] ramp by the particle's normalized age.

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
pub(crate) mod sim; // `SceneGates` is the ribbon sim's draw-set input too (decision 1294)

use emit::{emit_local, next_u32, rand01, rand_s11};
use sim::simulate_particles;
// The water-plane interleave classification — shared with the ribbon sim (a trail is one of the
// model's emitters and classifies the same way; `sky_order::FAR_SIDE_BIAS`).
pub(crate) use sim::{far_side_of_water, model_far_side, water_height, WaterInterleave};

/// This camera is **rate-throttled**: it renders on some frames and skips others *by our own
/// pacing*, while the scene behind it keeps running at full rate. A frame it does not draw is a
/// SKIPPED frame, not a sleeping scene — so the lanes that freeze with a sleeping booth camera
/// ([`sim::simulate_particles`]) must keep simulating through it.
///
/// The distinction is the whole point (decision 1559). The draw-set law — an emitter ticks only
/// inside a draw the frame performs, one frame's dt, no catch-up — is the *reference's*, and it
/// is keyed on the reference's own cull. `boothHalfRate` (decision 1444) is **ours**: a camera it
/// skips has a scene that is still posing at full rate (the skeleton's `AnimParked` keys on the
/// booth's LOGICAL active state, never on the rendered frame). Reading our skip as the
/// reference's cull ran the body panes' item emitters at half speed — director report B312,
/// which 1444's own record had flagged as the law's honest consequence and which is not: our
/// throttle must not reach the simulation at all.
///
/// Maintained on the booth camera entity by `portrait::gate_booth_cameras`, which owns both
/// halves of the state (the logical `active` and the physical `render`). It marks the *regime*,
/// not the parity — it is present for as long as the camera is being paced, on drawn frames and
/// skipped ones alike, so the archetype does not churn every frame.
#[derive(Component)]
pub struct ViewThrottled;

/// Hard cap on a single emitter's live particle count — a backstop against a pathological model. Real
/// props sit far under this (a campfire's steady state is `rate·lifespan` ≈ 30 + 24 particles).
const MAX_PARTICLES: usize = 1024;

/// Live particle tuning, driven by the debug panel. Holds only the reference's own
/// `particleDensity` CVar — a real game setting, not a dev knob. (The intensity/size/tint A/B
/// sliders — director-confirmed compensation for since-fixed source bugs, 0150/0152 — are gone;
/// the authored values are the only path.)
#[derive(Resource)]
pub struct ParticleTuning {
    /// The vanilla `particleDensity` CVar (byte-verified: handler `0x688fb0` clamps to
    /// [0.25, 1.0], the getter's only two callers are the spawn-count `fmul`s). Scales emission
    /// RATE only — never size, alpha, or draw distance. Default 1.0.
    pub(crate) density: f32,
}

impl Default for ParticleTuning {
    fn default() -> Self {
        Self { density: 1.0 }
    }
}

/// One live particle. Its frame is the emitter's storage space, which the file flag `0x10` picks
/// — and the two spaces are the whole of the ride-vs-trail law (decision 1585):
/// - **World mode** (flag `0x10` CLEAR — ~70 % of the corpus): `pos`/`vel` are **absolute world
///   coordinates, Bevy axes**. The birth bakes everything — bone pose, model rotation, scale,
///   position — through the live emitter matrix (`0x7b8acf`/`0x7b8b0f`), and the draw folds
///   **nothing** back (`0x7b3f48` omits `rt+0x1fc`). Once a particle exists, no later act of its
///   host can reach it: run, turn, or play any animation and the cloud hangs exactly where it was
///   born, which is what makes a trail of length `host speed × particle lifetime`. Gravity is
///   plain world down (Bevy `-Y`).
/// - **Model mode** (`0x10` SET): `pos`/`vel` are the emitter's **local WoW model space** (Z up)
///   and the draw re-applies the whole live placement transform every frame, rotation included
///   (`0x7b8aa5`/`0x7b3efb`) — the rigid ride a carried torch (`Club_1H_Torch_A_01.m2` = `0x0011`)
///   is flagged for, and the chandelier's flames riding its swing.
///
/// Storing world-relative coordinates and re-composing them against a live frame — which is what
/// this comment described until 1585 — cannot express the CLEAR case: it can be made to cancel a
/// host's translation, but its rotation still reaches every particle already born.
struct Particle {
    pos: Vec3,
    vel: Vec3,
    age: f32,
    /// This particle's lifetime, captured at birth from the emitter's **current sampled**
    /// lifespan channel (the reference passes it into each spawn as the kernels' `life_param`;
    /// the channel ANIMATES, e.g. Frost Nova 0.47 → 0.80 s, so a
    /// shared `def.lifespan` is wrong twice over). Kill at `age >= life`; over-life ramps
    /// normalize by it.
    life: f32,
    /// Spawn-time random phase for the twinkle LUT index (the reference hashes the particle's
    /// pointer; same role — de-sync the flicker across particles).
    phase: u32,
    /// The reference's particle+0xd bit 1 (set at spawn, cleared on the first integrate): a
    /// particle's first frame skips the follow-delta add (`0x7b2680`).
    fresh: bool,
    /// MODEL particles only (`0x7b2420`, integrator `0x7b28e0`): the instance orientation
    /// (stored frame; seeded from the birth fold) and its body-frame angular velocity — the
    /// integrator applies the Rodrigues half-angle spin whenever `angvel` is non-zero. Quad
    /// particles carry identity/zero.
    quat: Quat,
    angvel: Vec3,
}

impl ParticleEmitter {
    /// The rig bone this emitter is mounted on — the scope key the depth probes filter by
    /// (`$WOW_DEPTH_QUADS`, `$WOW_PARTICLE_DEPTHDUMP_BONES`).
    pub fn bone(&self) -> u16 {
        self.def.bone
    }
}

/// A spawned particle emitter: its parsed def + the placement that maps its local space to the world,
/// the live pool, the fractional emission accumulator, and a per-emitter RNG; its quads go into
/// the shared effect stream ([`buffer::EffectQuads`]) each frame. Despawns with its placement
/// (the entity joins the placement's entity list).
#[derive(Component)]
pub struct ParticleEmitter {
    def: ParticleEmitterDef,
    /// Model→world (Bevy space): `world = placement · wow_to_bevy(local)`. Carries the placement scale.
    /// Static for terrain doodads; refreshed each frame from [`Self::owner`] when set.
    placement: Transform,
    /// **The frame this cloud's WORLD-mode particles are stored in** — world on the ground, the
    /// transport's deck while the owning model rides one ([`crate::ride_frame`], decision 1591).
    ///
    /// The `0x10` CLEAR store bakes a birth into absolute coordinates and never re-applies a live
    /// matrix, which is what makes a trail. On a transport that leaves a rider's cloud behind the
    /// vehicle, so the reference divides the ride matrix out at birth (`rt+0x1fc = srcMx · A⁻¹`)
    /// and folds it back at draw (`0xcf5b68 = A · T · S`). MODEL-mode (`0x10` SET) emitters take
    /// the verbatim-copy arm and never see `A` at all — they already ride their host rigidly.
    pub(crate) ride: crate::ride_frame::StoredFrame,
    /// The entity this emitter belongs to, if it follows one (a streamed creature/GameObject). Each
    /// frame the emitter copies the owner's world transform into [`Self::placement`] (so it tracks a
    /// moving prop) and **drains** once the owner is gone. `None` for terrain doodads, which carry
    /// a fixed placement and despawn with their placement instead.
    owner: Option<Entity>,
    /// What losing [`Self::owner`] means for this emitter — the spawn site's call, because only it
    /// knows whether the owner entity going away is a MODEL BEING DESTROYED or an effect ending
    /// (see [`OwnerLoss`]).
    on_owner_loss: OwnerLoss,
    /// The owner died and this emitter is [`OwnerLoss::Drain`]: emission stops, the pool lives out
    /// its lifespans at the frozen placement, and the emitter despawns itself when empty — live
    /// particles finish instead of popping. The reference frees emitters synchronously at the
    /// model dtor (`0x70e313`), its fade coming from the model staying alive while emitters
    /// drain (the `HasLiveParticles` latch, `0x7b5f60`, keeps a disabled emitter ticking) —
    /// this drain reproduces that defer-until-drained shape from the owner side; see
    /// `ribbons::RibbonTrail::owner` for the one OPEN half.
    draining: bool,
    /// The model instance whose [`crate::model_fade::ModelAlpha`] this cloud is multiplied by —
    /// the reference's `emitter+0x1a8`, a per-frame copy of that model's `+0x19c` (`0x718960`
    /// @`0x719073`), folded into each particle's ALPHA by the over-life sampler (`0x7b9b10`
    /// @`0x7b9b42`). For an ATTACHED model — a held item, a helm, a pauldron — it is the
    /// WEARER's, because an attached model inherits its parent's computed alpha (decision 0827).
    /// `None` ⇒ 1.0: a placed doodad instead multiplies its own distance fade ([`EmitterFade`]).
    alpha_src: Option<Entity>,
    /// This frame's value of that alpha (1.0 until read).
    alpha: f32,
    /// The **cloud anchor**: the entity whose live translation is this pool's SORT point (the
    /// MODEL — a creature root, an effect-instance root, a held item's root), NOT the emitter's
    /// bone joint. Since 1585 it carries no particle position — a world-mode store is frozen at
    /// birth and a sort key is not a vertex — so this is a depth-ordering input only.
    /// `None` = sort at the spawn placement (a placed doodad whose emitter rides a joint sorts at
    /// the doodad, not the bone).
    anchor: Option<Entity>,
    /// The object's **light node** — the entity whose [`crate::interior::ParticleLight`] this
    /// cloud takes when it is LIT and standing in a WMO room. Held so a recursion CHILD that
    /// wires up lit under an unlit parent can still register the edge
    /// ([`wire_child_emitters`]); the [`crate::interior::EmitterLitBy`] component on this entity
    /// is what the classifier actually walks.
    light_node: Option<Entity>,
    /// The anchor's last-known world translation (kept when the entity vanishes so the pool
    /// drains in place; init = the spawn placement's translation).
    anchor_pos: Vec3,
    particles: Vec<Particle>,
    accumulator: f32,
    /// The emitter origin's last-frame live world position (the reference's rt+0x248 prevPos,
    /// refreshed EVERY frame — `0x7b5230` @0x7b5265): the one-frame Δ source for both
    /// emitter-motion terms (follow-delta, velocity inherit). `None` until the first frame.
    emitter_prev: Option<Vec3>,
    /// Velocity-inherit state (file flag 0x40, `0x7b5230`): the ~30 Hz
    /// trigger accumulator (rt+0x254) and the held inherit velocity (rt+0x258.., world frame) —
    /// recomputed only at a trigger, births read the held value between.
    inherit_accum: f32,
    inherit_vel: Vec3,
    /// Last frame's emission gate `(enabled && rate > 0)` — the rising-edge memory a BURST
    /// emitter (file flag 0x8000) latches on: it births its one `ftol(rate)` puff the frame this
    /// goes false→true and re-arms when it falls (the reference's `block+0x168`, `0x718f06`).
    /// Unused by continuous emitters.
    gate_prev: bool,
    /// Seconds since this emitter spawned — the clip clock the keyed emission-rate track samples
    /// against (an effect model's emitters spawn at its clip start, so age == clip time). Ambient
    /// props' constant tracks are age-invariant.
    age: f32,
    /// The emission clock's SEQUENCE source (`EmitTiming`, decision 0641's host resolution one
    /// channel over): the entity whose live `AnimationPlayer` names the playing sequence + clip
    /// time — a unit's / GameObject's emitters read the *playing* sequence's key window, so a
    /// quest object's explosion fires in its one-shot clips and stays OFF at idle. `None` = a
    /// pinned lane (placed doodad, effect rig, booth) on the spawn-age clock.
    host: Option<Entity>,
    /// The sequence FILE slot the timing samples: the pinned slot an effect's rig armed, or the
    /// host's last resolved slot (kept across frames where the host has no live player — the
    /// pre-arm frame reads that slot's opening pose). `None` ⇒ slot 0 (the doodad law).
    seq: Option<usize>,
    rng: u32,
    /// The OWNER model's reach from its origin in **world** yards (the authored
    /// [`ModelEmitter::owner_reach`] × the placement scale) — the draw-order rung this cloud and
    /// every CHILD cloud of it is biased by, so both clear the owner's own transparent batches
    /// (see [`owner_last_bias`]). Children carry the parent's, not their recursion model's: a
    /// child draws at the parent's anchor, so it is the parent's owner it has to clear.
    owner_reach: f32,
    /// The owner model's bound sphere (Bevy model-local centre, model-local radius) — the
    /// water-plane classification input ([`ModelEmitter::water_bound`]; the law is
    /// `sim::model_far_side`). Model-local: the live matrix supplies position and scale.
    water_bound: (Vec3, f32),
    /// The particle texture — the sim withholds pushing quads until it's resident: a
    /// still-loading or failed-to-decode texture would otherwise flash the engine fallback
    /// (white/magenta) through the additive blend. Particles still simulate meanwhile.
    texture: Handle<Image>,
    /// The draw-set gate's memory (was the emitter entity's `Visibility` flip): true while the
    /// owner is out of the frame's draw set — the edge on which model-instance entities are
    /// hidden. Quads need no flag: a gated pool simply pushes nothing into the shared stream.
    gated: bool,
    /// **The owner's own freeze** — the scene-level "this cloud is not being drawn" answer for a
    /// scene whose CAMERA cannot give it (decision 2046). [`sim::booth_frozen`] reads a
    /// booth-layered emitter's freeze off its camera's `is_active` bit, which is exact while one
    /// camera means one scene — and the `<Model>` tile atlas broke that: EVERY orthographic pane
    /// on the sheet shares one camera, and that camera is deliberately kept active whenever any
    /// cell is packed, so a pane that leaves the paint list still has a camera saying "drawn".
    /// The owner sets this instead, on the frame its own scene stops being drawn. The freeze it buys is exactly the
    /// booth's — pool + age HELD, one frame's dt on re-entry, no catch-up, no quads — and, like
    /// the booth's, it never applies to a [`Self::draining`] emitter, which has to run its pool
    /// out or leak for the session.
    frozen: bool,
    /// The pending recursion model (`0x7b5dd0`): once the asset resolves,
    /// [`wire_child_emitters`] turns its own emitters (cap 4, the reference's `0x7b5dfe`) into
    /// [`Self::children`] and clears this.
    recursion: Option<Handle<benilla_assets::M2Model>>,
    /// CHILD emitters — driven once per live parent particle per frame at the particle's
    /// position (never ambiently); each owns its pool and a [`ChildDraw`] mesh entity.
    children: Vec<ChildEmitter>,
    /// The GEOMETRY model (`0x7b1c80`, spawn driver `0x7b5550`): when authored, this emitter's
    /// particles render as 3-D instances of it instead of quads — [`model::update_model_particles`]
    /// grows/positions the instance pool below.
    geometry: Option<Handle<benilla_assets::M2Model>>,
    /// The live instance pool (one slot per drawn particle, grown on demand, hidden past the
    /// live count). Each slot's mesh entities carry per-instance tint-clone materials.
    model_instances: Vec<model::ModelInstance>,
    /// The lane's size unit for this cloud's unflagged particle half-extents —
    /// [`quads::DrawFrame::size_scale`]. `1.0` (a yard) for every world lane; a UI model tile
    /// sets its pixels-per-unit through [`Self::set_size_scale`].
    size_scale: f32,
    /// The render-target rectangle this cloud is confined to, target pixels — see
    /// [`Self::set_clip`]. `None` = the whole target (every world emitter).
    pub(crate) clip: Option<Vec4>,
}

/// One wired CHILD emitter (see [`ParticleEmitter::children`]): the recursion model's own
/// emitter def + texture and a private pool. Simulated entirely inside the parent's sim block
/// (no ECS ordering with the parent's fresh pool); its quads go into the shared stream as one
/// draw of its own, at the parent's anchor and rung.
struct ChildEmitter {
    def: ParticleEmitterDef,
    texture: Handle<Image>,
    particles: Vec<Particle>,
    accumulator: f32,
    gate_prev: bool,
    rng: u32,
}

/// Marker on a MODEL-particle instance entity ([`model`]) — keeps the sim's transform queries
/// provably disjoint (the owner-read query excludes these; the instance write query requires
/// it).
#[derive(Component)]
pub struct ChildDraw;

#[cfg(test)]
impl ChildEmitter {
    /// A bare child for unit tests.
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

/// What becomes of a live pool when its owner entity goes away — the distinction the sim itself
/// cannot make, because from inside it a despawned owner looks the same either way (decision 0826).
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum OwnerLoss {
    /// The owner MODEL was destroyed: **free the emitter with it**, live particles and all. This
    /// is the reference's own rule — a model's emitters are released synchronously at its dtor
    /// (`0x70e313`) — and it is what an equipped item that is
    /// replaced/unequipped, or a unit that streams out, must do. Draining instead left the cloud
    /// hanging in world space for a whole lifespan while the character walked away from it.
    Free,
    /// The owner was reaped but the effect should finish: emission stops and the pool lives out
    /// its lifespans at the frozen placement (a missile that impacted, a spell effect that
    /// ended — the reference's model outliving its emitters while they drain). The default,
    /// because it is what every pre-0826 caller got.
    #[default]
    Drain,
}

/// The live frames an emitter rides — bundled so a call site names each one instead of passing a
/// row of positional `Option<Entity>`s (they are easy to swap and impossible to tell apart at the
/// call). See the matching [`ParticleEmitter`] fields for what each frame does.
#[derive(Clone, Copy, Default)]
pub struct EmitterFrames {
    /// The entity whose live transform is this emitter's placement: `(entity, [0; 3])` for a
    /// streamed creature/GameObject/held item (the whole model rides), or `(joint, bone_pivot)`
    /// for an emitter riding its animated bone (0130 phase 4 — the pivot rebases the model-space
    /// `def.position` into the joint's own frame, so an unanimated chain reproduces the static
    /// path exactly). `None` for a static doodad (fixed placement, despawned by the placement's
    /// own entity list).
    pub owner: Option<(Entity, [f32; 3])>,
    /// The cloud anchor — the MODEL, never the bone ([`ParticleEmitter::anchor`]).
    pub anchor: Option<Entity>,
    /// The MODEL INSTANCE whose render alpha multiplies these particles
    /// ([`ParticleEmitter::alpha_src`]).
    pub alpha: Option<Entity>,
    /// The **light node** this cloud shades under ([`ParticleEmitter::light_node`]): the net
    /// entity root for a creature/GameObject, the WEARER for an equipped item (one node per
    /// object — the reference aliases the wearer's collector into each attached model). `None`
    /// for a lane with no entity light node of its own: a placed doodad or a WMO prop, whose
    /// provider is the CMapDoodadDef twin rather than the WENTITY, and a booth, whose light is
    /// its own buffer.
    pub light_node: Option<Entity>,
    /// What losing `owner` means ([`OwnerLoss`]).
    pub on_owner_loss: OwnerLoss,
}

/// Spawn an emitter entity for one [`ModelEmitter`] at `placement`. `None` if the emitter has no
/// resolved texture (nothing to draw); `frames` ties it to the live entities it rides.
impl ParticleEmitter {
    /// Live particle count — read by the perf probe ([`crate::capture`]).
    pub fn live(&self) -> usize {
        self.particles.len()
    }

    /// Set the lane's size unit for this cloud — see [`quads::DrawFrame::size_scale`]. A UI
    /// model tile (decision 2008) stores its cloud in device pixels and passes the reference's
    /// pixels-per-model-unit for a particle's half-extent; every world lane leaves the default.
    pub fn set_size_scale(&mut self, size_scale: f32) {
        self.size_scale = size_scale;
    }

    /// Freeze or thaw this cloud from the OWNER's side — see [`Self::frozen`]. A draining emitter
    /// ignores it.
    ///
    /// **Thawing clears [`Self::gated`]**, because the draw-set arm's cheap early-out
    /// (`gated && gate_inputs_still && !fade.is_changed()`, decision 1979's floor) reads none of
    /// the owner's inputs: a world-lane cloud left gated here on a still camera could never
    /// re-enter its own draw set. One full gate evaluation on the thaw edge is the price.
    /// **Confine this cloud's quads to a rectangle of the render target**, in target pixels —
    /// the UI model tiles' cell ([`crate::particles::buffer::EffectDrawSpec::clip`], decision
    /// 2093). `None` (the default, and every world emitter) draws over the whole target.
    ///
    /// A tile's cloud is quads in the shared atlas's own space, so without this a cloud that
    /// reaches past its cell lands in the cell the shelf packed beside it — which the composite
    /// hands to a different widget (B379: the autocast shine's golden `GlowStar` down the left
    /// edge of a bag slot's cooldown). The reference has no atlas and no such reach: it draws
    /// each `<Model>` with the widget's rect as the VIEWPORT.
    pub fn set_clip(&mut self, clip: Option<Vec4>) {
        if self.clip != clip {
            self.clip = clip;
        }
    }

    pub fn set_frozen(&mut self, frozen: bool) {
        self.frozen = frozen;
        if !frozen {
            self.gated = false;
        }
    }

    /// Is this cloud owner-frozen? ([`Self::set_frozen`]) — so an owner writing the same value
    /// every frame need not touch the component and trip change detection.
    pub fn is_frozen(&self) -> bool {
        self.frozen
    }

    /// The authored def — read by the particle census probe ([`crate::capture`]), which prints
    /// per-emitter facts (blend, texture, rate keys) beside the live count.
    pub fn def(&self) -> &ParticleEmitterDef {
        &self.def
    }

    /// The particle texture handle — read by the phase probe ([`crate::capture`]) to report the
    /// GPU-side image state beside the phase membership (a main-world-resident image whose GPU
    /// prep failed leaves every draw silently skipped).
    pub fn texture(&self) -> &Handle<Image> {
        &self.texture
    }

    /// The owner-last draw-order rung this emitter's effects take (see [`owner_last_bias`]).
    ///
    /// Read by the MODEL-particle draw ([`model::update_model_particles`]), whose instances are
    /// built from the geometry model's own batch materials and would otherwise sit at rung 0 — so
    /// a 3-D particle would be painted over by its own sibling quad cloud. The reference draws all
    /// of a model's emitters in the one bracket; sharing this number is what says so.
    pub(super) fn owner_rung(&self) -> f32 {
        owner_last_bias(self.owner_reach)
    }

    /// Is this emitter in the frame's draw set — i.e. did the draw-set gate let it tick and push
    /// quads this frame? The instrument-side spelling of [`Self::gated`], read by the particle
    /// census probe. (It read the entity's `Visibility` until slice P2 (0733) moved the family onto
    /// the shared stream and emitter entities stopped carrying one — after which the probe's query
    /// matched NOTHING and every column it printed, including B39's `drawn_beyond_wall` guard, was
    /// a vacuous zero.)
    pub fn drawn(&self) -> bool {
        !self.gated
    }

    /// This frame's MODEL render alpha ([`Self::alpha_src`]) — read by the MODEL-particle lane,
    /// whose instances carry their alpha in a `MeshTag` rather than a vertex colour.
    pub fn render_alpha(&self) -> f32 {
        self.alpha
    }

    /// The **cloud anchor** this emitter was spawned under ([`EmitterFrames::anchor`]) — the model
    /// instance it belongs to. The identity an effect lane uses to find its own emitters when the
    /// instance ends ([`Self::drain_on_owner_loss`]).
    pub fn anchor(&self) -> Option<Entity> {
        self.anchor
    }

    /// The **emission clock's host** ([`EmitClock::Host`]) — the model instance whose live
    /// `AnimationPlayer` names the sequence this emitter samples its rate/gate tracks against;
    /// `None` for a pinned or effect clock. The read an instrument or a wiring test needs, because
    /// the difference is invisible in every other observable: a hosted and a pinned emitter build,
    /// pool, tick and draw identically, and differ only in *which* sequence's tracks they read —
    /// which is the whole difference between a firearm's muzzle blast firing and never firing
    /// (decision 2281).
    pub fn emit_host(&self) -> Option<Entity> {
        self.host
    }

    /// Switch this emitter to [`OwnerLoss::Drain`] — **the effect is ending, so its already-emitted
    /// particles must finish rather than pop**.
    ///
    /// The reference makes this distinction the same way, at the same moment. `0x6203e0` — the
    /// teardown every ending `CEffect` reaches, whether by its completion callback or by the
    /// same-slot replace — does **not** free the node: it pushes it onto the pending-destroy list
    /// and hides it, which stops emission only. The particle draw path never reads a flag that
    /// teardown clears (its own admission test is the live-particle count `emitter+0x64`,
    /// `0x7b4b46`), so the survivors keep drawing and ageing out one at a time; `0x61f680` frees
    /// the node on the first frame the aggregate `CM2Model+0x3d8` reads zero — **the free
    /// condition and the draw condition are the same word**.
    ///
    /// It is deliberately NOT the spawn-time policy for an attached instance, because the *other*
    /// way an effect's owner can vanish is the model dtor — a gear change, a display swap, a unit
    /// streaming out — and there the reference frees the emitters synchronously and the pool must
    /// go with the body (decisions 0826/0833: draining that case stranded ghost clouds in the air
    /// where the character had been). Both look identical from inside the sim, so the ending side
    /// says so explicitly.
    pub fn drain_on_owner_loss(&mut self) {
        self.on_owner_loss = OwnerLoss::Drain;
    }

    /// The cloud's live world anchor — the census probe's fallback distance subject for an emitter
    /// with no [`EmitterFade`] (entity-owned: creatures, GameObjects, spell kits). A faded
    /// emitter's distance is measured to its OWNER's sphere instead, because that sphere is what
    /// the draw-set gate actually tests.
    pub fn anchor_world(&self) -> Vec3 {
        self.anchor_pos
    }

    /// The cloud-orientation fingerprint — read by the particle census probe: the live cloud's
    /// WORLD centroid, best-fit plane normal (unit), RMS thickness along it, and RMS in-plane
    /// radius, from the same composition the quad expansion draws with. The numeric "which way
    /// is this cloud actually facing" instrument: an orientation bug is settled by this number,
    /// never by eyeballing a capture (method: timing/geometry are measured). `None` under 4
    /// particles (no stable plane).
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
        // 3×3 covariance; the normal is its smallest-eigenvalue eigenvector — found by power-
        // iterating for the two largest principal axes and crossing them (robust enough for a
        // probe; exactness is not the point, the axis label is).
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

/// Ties a terrain-doodad emitter to its owner doodad's byte-verified distance-fade law
/// (`model_fade::doodad_fade_alpha`, `FUN_00683f80`): when the owner falls out of the draw set
/// (fade ≤ 0 — a small prop past 50 yd, a mid prop past 125), the emitter neither simulates nor
/// draws — the reference ticks particles as part of the owning model's animate step, which only
/// runs for drawn models. This is the system that bounds the reference's emitter population; the
/// particle-side rule (emission LOD, no cull — decision 0151) applies only WITHIN the draw set.
/// Attached by the terrain spawn path; entity-owned emitters (creatures/GameObjects/spells) are
/// bounded by server visibility instead and carry no fade.
/// The rule is shared by BOTH emitter families — the quad clouds here and the ribbon trails in
/// [`crate::ribbons`], which take this by value. `Clone` so one call site can build a placed
/// model's gate ONCE and hand the same value to both, rather than two constructions that can drift.
#[derive(Component, Clone)]
pub struct EmitterFade {
    /// World bounding-sphere radius of the OWNER doodad (selects the fade band).
    pub radius: f32,
    /// The owner doodad's world bbox centre — the fade measures horizontal distance to this
    /// (matching the doodad's own gate in `debug_panel::visibility`), not the emitter position.
    pub center: Vec3,
    /// The WMO placement whose doodad set this emitter's owner belongs to; `None` for an ADT map
    /// doodad. Read only by the exterior-window term: a prop of the building the camera is standing
    /// in is not exterior scene to itself, so its candle keeps burning (decision 0784's exemption,
    /// which the owner submesh gets through `WmoGroupVis` — an emitter carries no such component,
    /// so it carries the instance directly).
    pub(crate) instance: Option<Entity>,
    /// The **rooms** of [`Self::instance`] that name this owner — the prop's own
    /// [`crate::wmo_portal::WmoGroupVis`], by value.
    ///
    /// It answers the question [`Self::instance`] cannot: not *which building am I furniture of*
    /// (identity, for the exemption above) but *is the room I stand in drawn this frame*. The
    /// reference never has to ask, because it instantiates a WMO's props out of each **visible**
    /// group's own MODR list (`0x695aa0` from the visible-group walk `0x698720`, decision 0689) —
    /// a prop in a culled room is never created, so it has no emitters to tick. We create props
    /// once and cull them per frame, so every rider of a prop has to ask for itself, and this is
    /// the one place the answer is spelled.
    ///
    /// `None` = not a building's prop (an ADT map doodad, a creature, a GameObject, a spell kit),
    /// or a prop no group names at all — nothing claims it, so nothing gates it here. When it is
    /// `Some`, its `instance` is the same entity as [`Self::instance`]: both are built from one
    /// expression at one call site (`terrain_stream::spawn`), which is what keeps them from
    /// drifting.
    ///
    /// By value and not as a component on the emitter entity, because a `WmoGroupVis` there would
    /// enlist it in `apply_model_visibility`'s `group_only` query — a second `Visibility` writer on
    /// an entity whose `Visibility` this lane does not read (decision 0025).
    pub(crate) room: Option<crate::wmo_portal::WmoGroupVis>,
}

impl EmitterFade {
    /// A bare fade **sphere** — the gate of a placement that is nobody's furniture: an ADT map
    /// doodad, a WMO root's own geometry, a prop on a moving transport. `center` is WORLD space,
    /// already placed. The building-prop form goes through
    /// [`crate::terrain_stream::emitter_fade`], which composes this with the instance and rooms.
    pub(crate) fn sphere(radius: f32, center: Vec3) -> Self {
        Self {
            radius,
            center,
            instance: None,
            room: None,
        }
    }

    /// This owner's distance-fade ALPHA (not the cutoff): the reference writes it into the
    /// doodad's `CM2Model+0x180`, so it multiplies that model's particles exactly as it does its
    /// batches (`FUN_00683f80` → `+0x180` → `+0x19c` → `emitter+0x1a8`, decision 0827). The
    /// [`Self::in_draw_set`] gate is the same curve's zero crossing — this is the feather before
    /// it, which is why a small prop's flame now thins out over its band instead of cutting.
    pub fn distance_alpha(&self, cam_pos: Vec3) -> f32 {
        let (dx, dz) = (self.center.x - cam_pos.x, self.center.z - cam_pos.z);
        crate::model_fade::doodad_fade_alpha(self.radius, (dx * dx + dz * dz).sqrt())
    }

    /// The **draw-set admission rule** in one place: is the owner doodad in the frame's scene
    /// worklist, and therefore does its emitter tick and draw this frame?
    ///
    /// Five terms, the first four against the owner's fade sphere (the reference's `[rec+0x68]`):
    /// 1. the radius-tiered distance fade hasn't reached zero (`FUN_00683f80`),
    /// 2. the sphere is inside the far-clip wall ([`crate::view::within_farclip`]),
    /// 3. `lateral_in_frustum` — the caller's frustum sphere test on the side/near planes,
    /// 4. `exterior_admitted` — the caller's exterior-window test (decision 0786),
    /// 5. `room_admitted` — the caller's portal-PVS test ([`Self::room_admitted`], decision 0689).
    ///
    /// Term 2 is the one bug B39 was missing (decision 0678), and it cannot be folded into term 1:
    /// [`crate::model_fade::doodad_fade_alpha`] returns a flat `1.0` for any owner bigger than
    /// [`crate::model_fade::NEVER_FADE_RADIUS`], so for exactly the props that carry the big
    /// effects — braziers, bonfires, portal frames — term 1 admits at *every* distance.
    ///
    /// Term 4 is the same omission one layer out. The reference links a doodad into the worklist
    /// through the per-window populate walk `0x683700` and ticks its particles as part of that
    /// model's animate step — so an unlinked doodad emits nothing. We gated the doodad's *mesh* and
    /// left its emitter running, which is a mushroom's spore cloud hanging in a sealed dungeon (the
    /// director's report: `plaguelandmushroom01`, 3 emitters, 59.7 yd through a wall).
    ///
    /// Term 5 is the same omission one layer *in*: the exterior window asks whether the BUILDING is
    /// in the scene, and answers "yes" for every building the camera can see — which says nothing
    /// about the sealed room inside it that this emitter's owner actually stands in. Ninety Caverns
    /// of Time ribbon trails burned up through 200 yd of Tanaris rock on exactly that gap
    /// (decision 1289); this is the shared spelling that decision named as the fix.
    ///
    /// Pure and total so the rule is pinned by tests without an ECS (the `model_fade` pattern).
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

    /// **Term 5 for this emitter, given the live placement its rooms belong to** — kept beside the
    /// rule for the same reason [`Self::exterior_admitted`] is, so the particle sim, the ribbon sim
    /// and the meshless-host anim gate ask one question instead of three. The arms, and why the
    /// last one fails CLOSED, are [`crate::wmo_portal::room_admits`] — shared with the light lane,
    /// which rides the same rooms for the same reason.
    pub fn room_admitted(&self, instance: Option<&crate::wmo_portal::WmoPortalInstance>) -> bool {
        crate::wmo_portal::room_admits(self.room.as_ref(), instance)
    }

    /// Term 4 for this emitter, given the frame's gate and the placement the camera is inside. Kept
    /// beside the rule so both callers — the particle sim and the meshless-host anim gate — ask the
    /// same question instead of each re-deriving the exemption.
    pub fn exterior_admitted(
        &self,
        gate: &crate::exterior_cull::ExteriorGate,
        camera_instance: Option<Entity>,
    ) -> bool {
        if self.instance.is_some() && self.instance == camera_instance {
            return true; // a prop of the building the camera stands in — not exterior to itself
        }
        gate.admits_sphere(self.center, self.radius)
    }
}

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
    // A quad emitter with no resolved texture draws nothing; a GEOMETRY (model-particle)
    // emitter never draws quads — the default handle stays non-resident, keeping its quad
    // mesh permanently empty while the instances render.
    let texture = match emitter.texture.clone() {
        Some(t) => t,
        None if emitter.geometry.is_some() => Handle::default(),
        None => return None,
    };
    let mut def = emitter.def.clone();
    let owner = frames.owner.map(|(entity, pivot)| {
        // Rebase the model-space emitter origin into the owner frame (bone-local for a joint owner;
        // pivot = 0 leaves it model-space for a whole-model owner). Same raw WoW axes throughout —
        // the sim composes `def.position` with the owner's live transform at each birth.
        def.position = [
            def.position[0] - pivot[0],
            def.position[1] - pivot[1],
            def.position[2] - pivot[2],
        ];
        entity
    });
    // Gate on the rate track's PEAK over every sequence, not its first key: a one-shot burst
    // emitter (the blood spurt's starflash/glowball, 0140 fold-back) keys `0 → 200 → 0` — value[0]
    // is 0 but it absolutely emits.
    if def.params.peak_lifespan() <= 0.0 || def.timing.peak_rate() <= 0.0 {
        return None; // emits nothing
    }
    // The starting slot is the model's **loader-idle** sequence, never "unknown" (decision 0936).
    // The reference arms that sequence on every M2 instance at load, so an emitter always has a
    // sequence to sample; leaving it `None` handed the slot resolution to `EmitTiming::idx`'s
    // `unwrap_or(0)` degrade, which is the idle slot only by accident. On the Spawn/Stand/Despawn
    // GameObjects slot 0 is the *Spawn* flourish, so a content-gated instance (nothing armed, so
    // `playing_seq` never overrides this seed) sat at that slot's opening frame for ever — the
    // Stormwind battlefield banner firing its spawn sparkle-tails on a loop.
    let idle = Some(emitter.idle_seq);
    let (host, seq) = match clock {
        EmitClock::Pinned => (None, idle),
        EmitClock::Effect(s) => (None, s.or(idle)),
        EmitClock::Host(h) => (Some(h), idle),
    };
    // The owner's reach is model-local; the rung is a view-space distance, so it takes the
    // placement scale with it (a scaled-up creature's batches spread proportionally).
    let owner_reach = emitter.owner_reach * placement.scale.max_element();
    // Seed the RNG from the placement position so two campfires don't flicker in lockstep.
    let t = placement.translation;
    let rng = (t.x.to_bits() ^ t.y.to_bits().rotate_left(11) ^ t.z.to_bits().rotate_left(22))
        .wrapping_mul(0x9E37_79B9)
        | 1;
    // A LIT emitter is a consumer of its object's light node, exactly as a lit mesh batch is —
    // and it is frequently the ONLY one (all five of the Onyxia lava trap's mesh batches are
    // unlit). Registering the edge here is what puts the object in front of the interior
    // classifier at all; see [`crate::interior::EmitterLitBy`].
    let lit_node = frames.light_node.filter(|_| def.lit);
    let mut spawned = commands.spawn((
        // The sim writes the anchor here each frame — the census/phase instruments'
        // read point (the draw's own sort key rides the draw record instead).
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

/// The **owner-last** draw-order rung: how far to bias one of a model's EFFECTS past that model's
/// own transparent batches (`Transparent3d` sort key = view-space z **+ depth_bias**, ascending, so
/// a positive bias draws LATER — the sign law lives in [`crate::sky_order`]).
///
/// The reference draws a model's emitters in their own push/pop bracket (`0x70d8b0`) *after* that
/// model's batches, unconditionally — never interleaved with them. Bevy has one distance-sorted
/// transparent list, and an effect mesh carries `NoFrustumCulling`, which suppresses the `Aabb`
/// Bevy would otherwise sort by, so it sorts at its entity translation while each body batch sorts
/// at its own bind-pose AABB centre a yard or two up the model. Both measured shapes of that:
///
/// - **Quad clouds** sort at the owner's ORIGIN, so the order flips with camera ELEVATION. On the
///   voidwalker at 12 yd: at 0° the eye emitters land at transparent slots 324/325 with 5 of the 14
///   blend batches still behind them; at 35° they drop to slots 7/8 and **all 14** draw over them —
///   B16, the eye glow visible from below the eye horizon and gone from above (decision 0719).
/// - **Ribbon trails** sort at the live head node, which sits *inside* the owner, so they interleave
///   at every angle and the interleave MOVES. On the wisp at 6 yd, elevation 0°: its three streamers
///   land at slots 310/317/324 among the wisp's own 14 blend batches, 6 batches deep each, and which
///   batches are over which streamer changes frame to frame as the streamers whip (decision 0721).
///
/// `reach` is the owner's authored [`benilla_assets::ModelEmitter::owner_reach`] (or
/// [`benilla_assets::ModelRibbon::owner_reach`]) in **world** yards — the model-local bound
/// [`benilla_formats::m2_owner_reach`] measures, times the placement scale. View-space z is
/// 1-Lipschitz in world position and no owner batch sorts farther than `reach` from its origin, so
/// `z(origin) + reach ≥ z(any of its batch centres)`.
///
/// Every effect of one model must take the SAME rung, or the law is broken from the other side: the
/// reference draws a model's emitters in file order within the bracket, so a quad cloud on rung 4
/// beside a model-particle instance on rung 0 would paint over its own sibling.
///
/// The rounding, the ceiling and their reasons live with the shared implementation in
/// [`benilla_formats::owner_last_rung`] — the survey tools (`emdump`, `benilla-extract
/// fxordercensus`) print the same number this applies, and a rung computed twice drifts twice.
pub(crate) fn owner_last_bias(reach: f32) -> f32 {
    benilla_formats::owner_last_rung(reach)
}

/// Wire pending CHILD emitters: once a parent's
/// recursion model resolves, its own particle emitters — **capped at 4** (`0x7b5dfe`) — become
/// the parent's children, each with its private pool (drawn from the shared stream at the
/// parent's anchor and rung). The reference wires at the child model's async-load completion
/// (`0x7b5dd0`); this system is that completion hook. A child never self-emits ambiently — its
/// only particle source is the per-parent-particle drive in the sim.
/// An emitter whose child-emitter model (`ParticleEmitter::recursion`) has not been resolved
/// yet — the only emitters [`wire_child_emitters`] visits. Without it the wiring walked every
/// resident emitter every frame to find the handful still waiting on a model (decision 1979).
#[derive(Component)]
pub(crate) struct PendingChildren;

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
        // A child emitter is a whole emitter record of the recursion model with its own flag word,
        // so it takes its own lighting verdict (the same rule its texture/blend/fog identity
        // follows). A LIT child under an UNLIT parent is therefore a light-node consumer whose
        // edge nothing registered at spawn — the parent's `def.lit` was false and the child did
        // not exist yet. This is that late registration.
        if !registered && emitter.children.iter().any(|c| c.def.lit) {
            if let Some(node) = emitter.light_node {
                commands
                    .entity(entity)
                    .insert(crate::interior::EmitterLitBy(node));
            }
        }
    }
}

/// Which sequence clock an emitter's rate/enabled tracks sample against
/// ([`benilla_formats::EmitTiming`] — one baked loop per sequence, so the consumer names the
/// sequence). Callers pick per lane at [`spawn_emitter`].
#[derive(Clone, Copy, Default)]
pub enum EmitClock {
    /// Slot 0 on the spawn-age clock — a placed doodad's one-time arm (the pre-per-sequence law,
    /// still correct there), the portrait booths, and any lane with no rig to ask.
    #[default]
    Pinned,
    /// The SPELL-FX lane: the rig's armed slot (a missile's InFlight is not file-order-first;
    /// `None` = slot 0) on the spawn-age clock, like every pinned lane — the effect instance is
    /// byte-verified fresh per play (decision 0858), so its gseq loops open at phase 0 with it.
    Effect(Option<usize>),
    /// A live host's `AnimationPlayer` decides the slot **and the clip time** each frame — units
    /// and GameObjects, whose playing sequence changes; the reference's animate kernel
    /// (`0x714260`) samples the CURRENT sequence record for its emitter phase.
    Host(Entity),
}

/// One frame of the emission front end: how many births the pool is owed, loaded into
/// `accumulator` (fractional; the caller's birth loop drains whole particles). The reference's
/// per-frame emitter pass + spawn driver (`0x718960` / `0x7b5550`):
///
/// - `rate`/`emitting` arrive already sampled from the playing sequence's key window
///   ([`benilla_formats::EmitTiming`]) and the gate is `(enabled && rate > 0)`.
/// - A **continuous** emitter (file flag 0x8000 clear) pours `rate · density·LOD · dt`.
/// - A **BURST** emitter (flag set) loads `ftol(rate · density·LOD)` ONCE on the gate's rising
///   edge and re-arms when it falls (a looping clip's next pass re-fires it); the pour never
///   runs for it. This is what bounds the Feint/Eviscerate impact's plume+crescents at one puff
///   ~67 ms in — over by ~0.5 s — while the same-shaped cast-hands flame pours its whole clip.
///
/// Returns the burst count loaded this frame (0.0 otherwise) — the `fx` trace's measurement hook.
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

/// Registers the per-frame particle simulation/billboard system. Emitters are spawned by the terrain
/// streamer (which owns placement lifecycles) via [`spawn_emitter`].
pub struct ParticlePlugin;

impl Plugin for ParticlePlugin {
    fn build(&self, app: &mut App) {
        // The whole effect family draws through the dedicated lane
        // ([`render::EffectLanePlugin`]); `WowParticleMaterial` and its MaterialPlugin retired
        // with slice P2 (0733 §1) when precipitation and water foam moved onto the stream.
        app.add_plugins(render::EffectLanePlugin)
            .init_resource::<ParticleTuning>()
            .init_resource::<buffer::EffectQuads>()
            // PostUpdate, after the billboard joint palette: an emitter riding a billboarded
            // bone must sample the REPLACED frame, same frame — an Update-time read gets the
            // un-billboarded pose back (avian's fixed-loop sync re-propagates from locals), and
            // the Demon Skin flames followed the character instead of the camera. This also
            // retires the one-frame lag emitters on ANY animated joint carried while reading
            // last frame's globals from Update.
            //
            // `begin_effect_frame` clears the shared stream before BOTH writers (the ribbon sim
            // is the other), so the writer order between them stays free.
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
                    // …and after the CARD facing pass, for the same reason one step further out:
                    // an equipped item's emitter rides a mesh-less billboard *frame* card (its
                    // bone chain reaches a billboard bone and an item model has no rig to carry
                    // the palette replacement — decision 0813), so that card's transform has to be
                    // this frame's before births read it.
                    .after(crate::billboard::face_billboards),
            )
            // The write-order tripwire's re-arm, once the frame's stream has been extracted
            // (see `buffer::EffectQuads::cleared_this_frame`).
            .add_systems(Last, buffer::clear_effect_frame_flag);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use benilla_formats::ParticleShape;

    /// The minimal def, exposed for sibling-module tests (the sim's child-drive test).
    pub(crate) fn plain_def() -> ParticleEmitterDef {
        super::emit::tests::def(ParticleShape::Plane)
    }

    /// A spawnable emitter whose owner model's loader-idle sequence is file slot `idle_seq`.
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

    /// Spawn one emitter through [`spawn_emitter`] and report the sequence slot it was seeded with.
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

    /// **The emitter's opening slot is its model's loader-idle sequence, not file slot 0.**
    ///
    /// A Spawn/Stand/Despawn GameObject (`DuelingFlag`-shaped — the battlefield banners, the
    /// goobers, `ArenaFlag`) keeps its idle in slot 1; slot 0 is the Spawn flourish. Seeding `None`
    /// handed the resolution to `EmitTiming::idx`'s `unwrap_or(0)` degrade, so a rig-less instance —
    /// one whose constant-pose idle made the render gate skip the arm, so nothing ever overrode the
    /// seed — sampled the Spawn gate at its opening frame for ever. That is the Stormwind
    /// battlefield banner firing its spawn sparkle-tails on a permanent loop.
    ///
    /// Every clock takes the seed: `Host` because its player may arm nothing, `Pinned` because there
    /// is no rig to ask at all, `Effect(None)` because an unarmed effect rig rests on the same idle.
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
        // An effect rig that DID arm a slot still names it — the seed is a default, not an override.
        assert_eq!(
            seeded_slot(model_emitter(1), EmitClock::Effect(Some(2))),
            Some(2)
        );
        // The ordinary model, whose idle IS slot 0, is unchanged.
        assert_eq!(seeded_slot(model_emitter(0), EmitClock::Pinned), Some(0));
    }

    /// **The ending side asks for the drain; the spawn-time policy does not give it**
    /// (`0x6203e0` hides the node; `0x61f680` frees it once drained). An attached instance is
    /// spawned `Free` so that a model
    /// dtor — a gear change, a display swap, a unit streaming out — takes its pool with the body
    /// (0826/0833: draining that case stranded ghost clouds in the air). The *other* way an
    /// effect's owner vanishes is the effect simply ending, and there the reference hides the node
    /// and lets the particles age out. Both look identical from inside the sim, so the ending side
    /// flips the policy explicitly — and this is the flip.
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

    /// Camera at the origin looking down −Z (Bevy's convention), and an owner sphere `depth` yd
    /// straight ahead — the geometry every draw-set test below uses.
    fn gate(radius: f32, depth: f32, farclip: f32) -> bool {
        EmitterFade {
            radius,
            center: Vec3::new(0.0, 0.0, -depth),
            instance: None,
            room: None,
        }
        .in_draw_set(Vec3::ZERO, Vec3::NEG_Z, farclip, true, true, true)
    }

    /// **The owner-last rung, pinned** (decisions 0719/0721), through the wrapper the renderer
    /// actually calls. The whole point of it is one inequality — the rung must be STRICTLY greater
    /// than the owner's reach, or a batch centred at the edge of the model ties with the effect and
    /// the draw order goes back to being whatever the queue emitted. The voidwalker (reach 3.666 yd
    /// by the old vertex bound, 1.945 by the batch-centre one) is the measured case; the integer
    /// reach is the one that would tie under a bare `ceil`.
    #[test]
    fn the_owner_last_rung_always_clears_the_owner() {
        for reach in [0.0f32, 0.5, 1.0, 3.666, 4.0, 7.999, 12.0, 31.5] {
            let rung = owner_last_bias(reach);
            assert!(rung > reach, "rung {rung} must clear reach {reach}");
            assert_eq!(rung, rung.trunc(), "rung {rung} must be a whole yard");
        }
        // A model too big for the ladder is capped rather than allowed to climb into
        // `sky_order`'s rungs — it loses its own ordering, not the world's.
        assert_eq!(owner_last_bias(500.0), 32.0);
        assert_eq!(owner_last_bias(-1.0), 1.0);
    }

    /// **The B39 defect, pinned** (decision 0678). An owner bigger than `NEVER_FADE_RADIUS` never
    /// distance-fades, so the fade term admits it at every distance — the far-clip term is the only
    /// thing that stops its emitter. Reverting that term makes this test fail, which is the point:
    /// a big fire prop's flames used to draw a kilometre away, over terrain the same wall had
    /// already discarded.
    #[test]
    fn a_never_fading_owner_is_still_bounded_by_the_wall() {
        let big = crate::model_fade::NEVER_FADE_RADIUS + 5.0;
        // The fade term alone says "draw" at any distance — that is what made this invisible.
        assert_eq!(crate::model_fade::doodad_fade_alpha(big, 5000.0), 1.0);
        assert!(gate(big, 500.0, 777.0), "inside the wall: draws");
        assert!(!gate(big, 1000.0, 777.0), "past the wall: must NOT draw");
        // ...and it tracks the live farclip, not a constant: the same emitter at the vanilla
        // minimum view distance is out, at the panel's maximum it is back in.
        assert!(!gate(big, 500.0, 177.0));
        assert!(gate(big, 1000.0, 1200.0));
    }

    /// The wall never *replaces* the fade cutoff — a small prop still pops at its own band end
    /// (40→50 yd) far inside the wall. Both terms are live; neither subsumes the other.
    #[test]
    fn the_wall_does_not_replace_the_size_fade() {
        assert!(gate(0.3, 45.0, 777.0), "mid-band: still drawing");
        assert!(
            !gate(0.3, 60.0, 777.0),
            "past the 50-yd band end, nowhere near the wall"
        );
    }

    /// The lateral frustum term is ANDed, not implied: an owner dead ahead and well inside the
    /// wall is still out when the caller's frustum test rejects it (off-screen).
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
        // …and the exterior-window term is ANDed the same way (0786): a doodad no portal window
        // admits is not in the worklist, so its emitter neither ticks nor draws.
        assert!(!f.in_draw_set(Vec3::ZERO, Vec3::NEG_Z, 777.0, true, false, true));
        // …as is the room term (0689/1289): the window admits the BUILDING, the PVS admits the
        // ROOM, and a prop in a culled room is one the reference never instantiated at all.
        assert!(!f.in_draw_set(Vec3::ZERO, Vec3::NEG_Z, 777.0, true, true, false));
    }

    /// **The exemption, on the emitter lane.** A sealed room (no windows) admits no exterior scene
    /// — but the props of the building the camera is standing IN are not exterior to it, and their
    /// flames must keep burning. Getting this wrong does not look like a cull bug; it looks like
    /// walking into an inn snuffs its fireplace, which is why it is pinned rather than argued.
    ///
    /// The third case is the reported one: an ADT map doodad carries no building at all, so a
    /// sealed room must stop it (`plaguelandmushroom01`'s spore cloud, seen through a wall).
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
        // Outdoors the gate stands down and every one of them emits again.
        for instance in [Some(here), Some(elsewhere), None] {
            assert!(fade(instance).exterior_admitted(&ExteriorGate::Open, Some(here)));
        }
        // …and with no room claimed at all, `None == None` must NOT read as "my own building".
        assert!(!fade(None).exterior_admitted(&sealed, None));
    }

    /// **A prop in a culled room emits nothing, and an orphaned emitter refuses.**
    ///
    /// The reference instantiates a WMO's props out of each VISIBLE group's own MODR list
    /// (`0x695aa0` from the visible-group walk `0x698720`, decision 0689) — a prop in a culled room
    /// is never created, so it has no emitters to tick. Our props' *meshes* are culled by exactly
    /// this predicate and their emitters were not, which is how Caverns of Time's twelve
    /// energy-trail props kept ninety additive ribbon strips burning up through 200 yd of rock into
    /// Tanaris while every one of their submeshes was correctly hidden (decision 1289).
    ///
    /// All four arms, including the one that fails CLOSED where the rest of the cull fails open.
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

        // Not a building's prop at all — an ADT map doodad, a creature, a held item's streak.
        assert!(
            fade(None).room_admitted(Some(&cot)),
            "an unclaimed emitter is never gated"
        );
        // The room the flood reached draws; the ones it did not, do not.
        assert!(fade(Some(&[0])).room_admitted(Some(&cot)));
        assert!(!fade(Some(&[1, 2])).room_admitted(Some(&cot)));
        // A prop several rooms name draws while ANY of them is visible — the same `drawn_by` law
        // the submeshes take, which is the whole point of sharing the predicate.
        assert!(fade(Some(&[1, 0])).room_admitted(Some(&cot)));
        // …and the asymmetry that must NOT be the cull's usual fail-open: an emitter whose
        // placement has despawned refuses, rather than drawing one last frame through the hole
        // where a building used to be.
        assert!(
            !fade(Some(&[0])).room_admitted(None),
            "an orphaned emitter draws nothing"
        );
    }

    /// The emitter gate and the owner mesh's own cull must agree on the boundary — they read the
    /// same `within_farclip` now, and this pins that they cannot drift apart again. An emitter
    /// admitted where its owner's mesh is culled IS bug B39.
    ///
    /// Scoped to owners **above** `NEVER_FADE_RADIUS` on purpose: there the size-fade term is a
    /// constant `1.0`, so the gate reduces to the wall alone and the comparison is exact. Below it
    /// the two rules legitimately differ (the mesh keeps drawing while the fade feathers it out),
    /// so an equality there would be asserting something false — and these are the owners that
    /// carry the big effects the report is about anyway.
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

    /// The BURST emission model (file flag 0x8000 — Feint/Eviscerate impact's plume+crescents):
    /// one `ftol(rate·scale)` puff on the rising edge of `(enabled && rate > 0)`, latched while
    /// the gate holds, re-armed when it falls. (The rate/enabled *sampling* laws — step vs lerp
    /// vs held tail, per sequence window — are pinned in `benilla-formats`' `emit_timing` tests,
    /// where the bake lives.)
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
        // The gate falls (drain/disable) → re-arms → the next rise bursts again (a looping
        // clip's next pass), the count ftol-truncated through density·LOD.
        accumulate_emission(true, 30.0, false, 1.0, 0.016, &mut acc, &mut prev);
        assert_eq!(
            accumulate_emission(true, 30.0, true, 0.55, 0.016, &mut acc, &mut prev),
            16.0,
            "ftol(30 · 0.55) = 16"
        );
    }

    /// The continuous emission model (flag clear) pours `rate·dt` — and drops its owed
    /// fraction while disabled (the pre-existing drain hygiene).
    #[test]
    fn continuous_emitter_pours_rate_dt() {
        let (mut acc, mut prev) = (0.0, false);
        accumulate_emission(false, 30.0, true, 1.0, 0.1, &mut acc, &mut prev);
        accumulate_emission(false, 30.0, true, 1.0, 0.1, &mut acc, &mut prev);
        assert!((acc - 6.0).abs() < 1e-4, "30/s × 0.2 s, no burst latch");
        accumulate_emission(false, 30.0, false, 1.0, 0.1, &mut acc, &mut prev);
        assert_eq!(acc, 0.0, "disabled zeroes the owed fraction");
    }

    /// The follow-delta response line (`0x7b5d30`): the line through the two authored
    /// (speed, fraction) samples; equal speeds degenerate to no response (the reference zeroes
    /// slope and intercept).
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
