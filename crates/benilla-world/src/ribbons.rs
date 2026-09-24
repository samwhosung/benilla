//! Faithful M2 **ribbon trails** — weapon enchant trails, wisp streamers, spell-missile trails.
//!
//! The 1.12 client simulates a ribbon as a ring of **edges**: each frame the emitter's bone-local
//! origin is transformed by the live bone matrix into a node point, `floor(dt·edgesPerSecond +
//! phase)` new edges (a vertex pair at `±heightAbove/heightBelow` across the node) are committed
//! and backdated inside the frame, old edges age out at `edgeLifetime`, **gravity carries the
//! stored verts along world +Z by `g·t²`** — up for the positive majority of the corpus — and the
//! edge list renders as a triangle strip whose `u` texcoord slides with edge age (the texture's
//! transparent tail fades the trail). The sim below transcribes the reference's byte-exact
//! behaviour, with the same simplifications as `particles` (distributions and frames mirrored, not
//! the reference's exact float slots).
//!
//! Like particles, each trail writes its strip into the **shared effect-quad stream**
//! ([`crate::particles::buffer::EffectQuads`], decision 0732 slice P1) — per segment, one quad
//! duplicating the shared edge vertices (identical triangles to the old strip mesh; a few dozen
//! extra vertices per trail buys the whole family one vertex layout and one index pattern). A
//! trail rides its **owner** entity (a skinned model's host-bone joint, an item root, a
//! missile); when the owner goes it drains — committed edges finish fading, then the trail
//! despawns itself (the reference's enable-gate law).

use std::collections::VecDeque;

use benilla_assets::coords::wow_to_bevy;
use benilla_assets::ModelRibbon;
use benilla_formats::ParticleBlend;
use bevy::camera::primitives::{Frustum, Sphere as CullSphere};
use bevy::camera::Projection;
use bevy::prelude::*;

use crate::particles::buffer::{EffectDrawSpec, EffectFog, EffectQuads, EffectVertex};
use crate::view::WorldCamera;

/// Hard cap on stored edges — a backstop against a pathological rate·lifetime (the reference's
/// ring capacity is `ceil(rate·lifetime)+2`; shipped trails sit far below this).
const MAX_EDGES: usize = 512;

/// **What the ribbon lane did this frame** — live trails, and how many of them wrote a strip.
///
/// The parallel of [`crate::exterior_cull::ExteriorCullVerdict`], and here for the same reason: a
/// trail carries no [`crate::model_render::ModelPart`] and no
/// [`crate::particles::ParticleEmitter`], so it is invisible to the visibility census AND to the
/// particle census. Ninety Caverns-of-Time energy trails burning across a Tanaris hillside read,
/// on every instrument we had, as a scene with nothing in it.
#[derive(Resource, Default)]
pub struct RibbonVerdict {
    /// Trails alive this frame (streaming or draining).
    pub trails: usize,
    /// …and how many committed a strip into the shared effect stream.
    pub drawn: usize,
}

/// One committed trail edge: the vertex pair across the node, in the trail's **stored** frame
/// ([`RibbonTrail::ride`] — world on the ground, the deck on a transport), and its birth time on
/// the shared clock.
struct Edge {
    top: Vec3,
    bottom: Vec3,
    born: f32,
    /// Seconds this edge has been alive, carried explicitly because the reference does
    /// ([`simulate_ribbons`]'s gravity note): its per-edge age array at `emitter+0x0c` is the term
    /// the gravity increment reads, and an edge born part-way through a frame starts at its
    /// **backdated** age, not zero.
    age: f32,
}

/// This frame's gravity displacement for one live edge, in world **+Z (up)** yards — the
/// reference's exact per-frame term (`0x7b7e60`, `0x7b8007`..`0x7b800f`):
/// `gravity · ((age + age) + dt) · dt`, applied identically to both of the edge's vertices, with
/// `age` advanced by `dt` straight after.
///
/// `(2a + dt)·dt ≡ (a + dt)² − a²`, so stepping it across an edge's life telescopes exactly to
/// `Δz = gravity · t²` — no ½, no factor of 2, and **upward** for a positive `gravity` (the block
/// is two `fadd`s with no `fchs`, on world-space positions in a +Z-up frame). See
/// [`simulate_ribbons`] for why both halves of that mattered on screen.
fn gravity_step(gravity: f32, age: f32, dt: f32) -> f32 {
    gravity * ((age + age) + dt) * dt
}

/// A live ribbon trail riding `owner`. Positions are world-space; the mesh entity's transform is
/// identity (like a particle emitter's).
#[derive(Component)]
pub struct RibbonTrail {
    def: benilla_formats::RibbonEmitterDef,
    /// The emission origin in the owner's frame: `wow_to_bevy(position − bone_pivot)` for a joint
    /// owner (the same rig identity as particle emitters), `wow_to_bevy(position)` for a root.
    local_offset: Vec3,
    /// The node source — `None` once the owner is gone (missile impacted, effect reaped, item
    /// unequipped): the trail then **drains** — commits nothing, ages its edges out, and
    /// despawns itself with the last edge. The reference frees a model's emitters SYNCHRONOUSLY
    /// at the model dtor (`0x70e313` — no orphan list);
    /// its visible fade comes from keeping the MODEL alive while emitters drain (the
    /// `HasLiveParticles 0x7b5f60` latch + the model's is-any-emitter-active flag). Our owners
    /// despawn at their own moment (impact, reap), so this drain reproduces the
    /// defer-until-drained shape; whether the client's effect controller actually polls the
    /// active flag before destroy, or hard-cuts at animation end, is the one OPEN half
    /// (CEffect-side, flagged in decision 0206).
    owner: Option<Entity>,
    /// Which clock the `+0xc0` enable gate is sampled against, every frame ([`RibbonSeq`]).
    seq: RibbonSeq,
    /// The MODEL INSTANCE whose [`crate::model_fade::ModelAlpha`] decides whether this trail is
    /// drawn at all. The reference's ribbon render leg reads the owning model's
    /// render alpha (`block+0x3c × Model+0x19c`) and **drops the draw** below a threshold
    /// (`0x707680`) — so an invisible model has no streamer, which is what a
    /// first-person avatar's enchant trail needs (ledger F05). Only the drop is implemented: the
    /// note does not say the model alpha scales the strip's vertex colour the way it does a
    /// particle's, and inventing a ramp on top of a gate would be building past the evidence.
    /// `None` ⇒ always drawn (a placed prop, an effect instance).
    alpha_src: Option<Entity>,
    /// **Is this trail's model in the frame's scene at all?** — the placed model's own
    /// [`crate::particles::EmitterFade`], carried by value.
    ///
    /// A trail is one of its model's emitters, and the reference ticks and draws a model's
    /// emitters — quad clouds and ribbons alike — inside **that model's** draw step. So a trail
    /// takes exactly the gate the quad clouds beside it take, from the same construction at the
    /// same spawn site: distance fade, the far-clip wall, the lateral frustum, the exterior window,
    /// and the room ([`crate::particles::EmitterFade::in_draw_set`]). The ribbon lane used to have
    /// **no term of any kind** here, which is how ninety Caverns of Time trails burned up through
    /// 200 yd of Tanaris rock (the room term) and how a placed prop's streamer drew
    /// past the far-clip wall its own mesh had already been culled by (bug B39, decision 0678 —
    /// fixed for the quad clouds and left open here).
    ///
    /// By value and not as a component, because a trail entity holding an `EmitterFade` would be
    /// picked up by the particle sim's own optional-fade query, and its `WmoGroupVis` inside would
    /// enlist it in `apply_model_visibility`'s group query — a second `Visibility` writer on an
    /// entity whose `Visibility` nobody reads.
    ///
    /// `None` = an ENTITY-owned trail (a creature, a GameObject, a missile, a held item), bounded
    /// by server visibility and by [`Self::alpha_src`] instead — the same split
    /// [`crate::particles::EmitterFade`] itself takes, and with the same far-clip backstop applied
    /// in [`simulate_ribbons`] for the one lane server visibility does not bound (transports).
    fade: Option<crate::particles::EmitterFade>,
    /// **The frame the committed edges are expressed in** — world on the ground, the transport's
    /// deck while the owning model rides one ([`crate::ride_frame`]).
    ///
    /// A ribbon's whole point is that a committed edge never moves again, which is what draws a
    /// streak behind a swinging weapon. On a transport that same rule streams the streak off the
    /// back of the vehicle, so the reference stores the edges in the ride frame instead and
    /// re-projects them through its live pose at draw — the ribbon twins of the particle folds
    /// (birth `0x718dd8` → `0x7b76c0`, draw `0x70d87d` → `0x7b80c0`), and note the negative: the
    /// ribbon birth has **no `0x100` gate**, so a trail is ride-framed unconditionally where a
    /// particle cloud is only in world mode.
    ride: crate::ride_frame::StoredFrame,
    /// Committed edges, newest at the back, in [`Self::ride`]'s frame. The live head (the current
    /// node) is appended at render time only, so the trail always connects to the emitter between
    /// commits.
    edges: VecDeque<Edge>,
    accumulator: f32,
    /// Seconds since spawn — the clip clock the keyed look tracks (colour/alpha/heights) sample
    /// against (an effect model's ribbons spawn at its clip start, so age == clip time — the
    /// particle emitters' law; a persistent trail's constant tracks are age-invariant).
    age: f32,
    texture: Handle<Image>,
    /// The owner-last draw-order rung ([`crate::particles::owner_last_bias`] over the owner's
    /// world reach, computed at spawn) — a trail is one of its model's emitters and takes the
    /// SAME rung as the quad clouds beside it. Was the material's `depth_bias`; now the
    /// draw record's sort-key add.
    bias: f32,
    /// The owner model's bound sphere ([`ModelRibbon::water_bound`]) — the water-plane side is
    /// the MODEL's: the ribbon leg reads the model's side-A boolean verbatim, slack
    /// included, so the trail flips with its model's bound centre, never with its whipping head.
    water_bound: (Vec3, f32),
}

impl RibbonTrail {
    /// The emitter bone this trail rides — the identity `WOW_PHASE=particles:<bone>` arms on, and
    /// the one `emdump`/`m2anim` print, so an instrument line and an asset line name the same trail.
    pub fn bone(&self) -> u16 {
        self.def.bone
    }

    /// The committed strip in **world** space — the points the draw rasterizes, folded out of the
    /// store's ride frame ([`Self::ride`]). For instruments: the trail census measures this
    /// strip's spread inside the DECK's frame, which is exactly the observable the ride law is
    /// about ("does the streak hang still relative to the floor you're standing on?").
    pub fn strip_world(&self) -> impl Iterator<Item = Vec3> + '_ {
        self.edges
            .iter()
            .flat_map(move |e| [self.ride.to_world(e.top), self.ride.to_world(e.bottom)])
    }

    /// The transport this trail's edges are stored against — `None` on the ground.
    pub fn deck(&self) -> Option<Entity> {
        self.ride.source()
    }

    /// The authored edge lifetime (seconds) — the trail's own length in time, which is what a
    /// streak's world extent divides by to give the host speed that drew it.
    pub fn edge_lifetime(&self) -> f32 {
        self.def.edge_lifetime
    }

    /// The authored blend, and how many edges are committed right now (0 = nothing drawn yet).
    pub fn shape(&self) -> (ParticleBlend, usize) {
        (self.def.blend, self.edges.len())
    }
}

/// What decides a trail's `+0xc0` **enable** gate — the reference's per-ribbon `block+0xbc` byte,
/// which it re-reads every frame (`0x717660`).
#[derive(Clone, Copy)]
pub enum RibbonSeq {
    /// Re-read each frame from this entity's [`AnimationPlayer`] + [`ModelAnimations`] — the unit,
    /// GameObject, doodad and effect-instance lanes, where the sequence **changes under the
    /// trail**: a trap springs, a door opens, an effect model steps Stand → Hold → Decay.
    Host(Entity),
    /// A fixed `AnimationData.dbc` id for the instance's life — a worn item rests in `Stand`(0),
    /// and nothing on it will ever play anything else.
    Fixed(u16),
}

/// Spawn a ribbon-trail entity for one [`ModelRibbon`], riding `owner` (a host-bone joint for a
/// skinned model — pass the joint and the def's baked pivot does the rebase — or the model/item
/// root). `seq` names the clock the per-sequence enable gate is sampled against (see
/// [`RibbonSeq`]). `owner_scale` is the owner placement's largest scale component — the
/// model-local [`ModelRibbon::owner_reach`] takes it to reach world yards, which is what the
/// draw-order rung is measured in. `None` if the trail has no resolved texture or degenerate
/// emission.
///
/// A gated trail still spawns: the gate is **live**, not a spawn-time decision, because the
/// sequence that answers it changes while the instance lives. Refusing at spawn was correct only
/// for the fixed-sequence lanes it was written for (a held item, a missile) and silently wrong for
/// everything with a state machine — see [`simulate_ribbons`].
pub fn spawn_ribbon(
    commands: &mut Commands,
    ribbon: &ModelRibbon,
    owner: Entity,
    use_pivot: bool,
    owner_scale: f32,
    seq: RibbonSeq,
    alpha_src: Option<Entity>,
    fade: Option<crate::particles::EmitterFade>,
) -> Option<Entity> {
    // Perf-bisect kill-switch: $WOW_NO_PARTICLES also spawns no ribbons (one switch, whole family).
    if std::env::var_os("WOW_NO_PARTICLES").is_some() {
        return None;
    }
    let texture = ribbon.texture.clone()?;
    let def = ribbon.def.clone();
    // The degenerate gate reads the tracks' PEAKS: a keyed slash (HolySmite) is born at height 0
    // and flares mid-clip — its value[0] is exactly the zero this gate must not trip on.
    if def.edges_per_second <= 0.0
        || (def.height_above.peak().max(0.0) + def.height_below.peak().max(0.0)) <= 0.0
    {
        return None; // nothing to trail
    }
    let p = def.position;
    let local = if use_pivot {
        [
            p[0] - ribbon.bone_pivot[0],
            p[1] - ribbon.bone_pivot[1],
            p[2] - ribbon.bone_pivot[2],
        ]
    } else {
        p
    };
    Some(
        commands
            .spawn((
                // The sim writes the trail's sort anchor (the live head node) here each frame
                // — the phase probe's read point.
                Transform::IDENTITY,
                RibbonTrail {
                    local_offset: wow_to_bevy(local),
                    def,
                    owner: Some(owner),
                    seq,
                    alpha_src,
                    fade,
                    ride: crate::ride_frame::StoredFrame::default(),
                    edges: VecDeque::new(),
                    accumulator: 0.0,
                    age: 0.0,
                    texture,
                    // The reference's "a model's emitters draw after that model's batches" —
                    // the same rung the quad clouds take, from the same authored reach,
                    // because a trail is one of the model's emitters.
                    bias: crate::particles::owner_last_bias(ribbon.owner_reach * owner_scale),
                    water_bound: ribbon.water_bound,
                },
            ))
            .id(),
    )
}

/// Per-frame: place the node from the owner's live transform, commit/expire edges, sag by
/// gravity, and write the strip into the shared effect-quad stream.
pub(crate) fn simulate_ribbons(
    time: Res<Time>,
    mut commands: Commands,
    // Owner reads (joints/roots — never trail entities): disjoint from the trail query's
    // `&mut GlobalTransform` below.
    transforms: Query<&GlobalTransform, Without<RibbonTrail>>,
    // The `+0xc0` enable gate's clock: the sequence a [`RibbonSeq::Host`] instance is playing.
    hosts: Query<(&AnimationPlayer, &benilla_assets::ModelAnimations)>,
    images: Res<Assets<Image>>,
    mut quads: ResMut<EffectQuads>,
    // The owning model's render alpha — the trail's draw gate, composed along the
    // attached-model chain.
    model_alphas: crate::model_fade::ModelAlphas,
    // Trails belong to the world lane (no booth ribbons; a booth-parked owner's strip is eaten
    // by the shader's farclip wall, exactly as on the material path). The frustum/projection come
    // with it because the draw-set gate below needs the same lateral test the quad clouds take.
    world_cam: Query<(Entity, &GlobalTransform, &Frustum, &Projection), With<WorldCamera>>,
    // The water-plane interleave inputs — a trail is one of the model's emitters and classifies
    // above/below water like the quad clouds ([`crate::particles::far_side_of_water`]).
    interleave: crate::particles::WaterInterleave,
    // The draw-set gate's scene inputs — far-clip wall, exterior window, portal PVS — the SAME
    // bundle `simulate_particles` reads, so the two emitter families cannot answer differently
    // ([`RibbonTrail::fade`]).
    gates: crate::particles::sim::SceneGates,
    // Which transport (if any) each trail's MODEL is riding — the frame its edges are stored in.
    rides: crate::ride_frame::RideFrames,
    mut verdict: ResMut<RibbonVerdict>,
    mut trails: Query<
        (
            Entity,
            &mut RibbonTrail,
            &mut Transform,
            &mut GlobalTransform,
        ),
        // The camera's `&GlobalTransform` read above must be provably disjoint from these writes
        // (a trail never rides the camera entity) — the particle sim's emitter query says the same.
        Without<WorldCamera>,
    >,
) {
    *verdict = RibbonVerdict::default();
    let Ok((cam, cam_tf, frustum, projection)) = world_cam.single() else {
        return;
    };
    let cam_pos = cam_tf.translation();
    // The far-clip wall's axis — the same forward the owner mesh's own cull measures along, so a
    // trail and the model it belongs to cross the wall together.
    let cam_fwd = Vec3::from(cam_tf.forward());
    let (farclip, exterior_gate, camera_instance) = gates.scene(Some((cam_tf, projection)));
    let dt = time.delta_secs().min(0.1);
    let now = time.elapsed_secs();
    for (entity, mut trail, mut entity_tf, mut entity_global) in &mut trails {
        let RibbonTrail {
            def,
            local_offset,
            owner,
            seq,
            alpha_src,
            fade,
            ride,
            edges,
            accumulator,
            age,
            texture,
            bias,
            water_bound,
        } = &mut *trail;

        // Owner gone (despawned missile/creature, unequipped item root) → DRAIN: no new
        // commits, the committed edges age out, and the trail despawns with its last edge
        // (see [`RibbonTrail::owner`]).
        if owner.is_some_and(|o| !transforms.contains(o)) {
            *owner = None;
        }
        let head = owner.and_then(|o| transforms.get(o).ok()).map(|owner_gt| {
            let node = owner_gt.transform_point(*local_offset);
            // Cross-section axis: the bone frame's local +Y (`0x7b76c0` captures the basis fresh
            // each frame from the live bone matrix, row 1 (= bone-local +Y) being the
            // ±heightAbove/Below span (`0x7b6990` fmuls only that pair). Sampling the live owner
            // rotation here IS that per-frame capture. (First pinned by elimination on the fireball
            // missile's authored bone pair; the bytes then confirmed it.)
            let axis = (owner_gt.rotation() * wow_to_bevy([0.0, 1.0, 0.0])).normalize_or(Vec3::Y);
            (node, axis)
        });
        if head.is_none() && edges.is_empty() {
            commands.entity(entity).despawn();
            continue;
        }
        verdict.trails += 1;

        // **The ride frame, this frame** — the transport the owning MODEL is on, inherited down
        // the `ParentModel` chain (an enchant streamer's model is the weapon; the weapon's parent
        // is its wearer). A transport that has streamed out reads as no frame, which is the leave
        // leg: the edges are handed back to the world where they stood.
        let deck = alpha_src
            .or(*owner)
            .and_then(|model| rides.source(model))
            .and_then(|t| {
                transforms
                    .get(t)
                    .ok()
                    .map(|gt| (t, crate::ride_frame::ride_matrix(gt)))
            });
        // Boarding and leaving RE-EXPRESS the committed edges rather than snapping them (the
        // reference's `0x7187f0` → `0x7b7bc0` on the NULL↔non-NULL edge). Riding a *moving* deck
        // folds nothing at all — the draw's live `A` is what carries the strip, and touching the
        // store as well would ride it twice.
        if let Some(fold) = ride.retarget(deck) {
            for e in edges.iter_mut() {
                e.top = fold.transform_point3(e.top);
                e.bottom = fold.transform_point3(e.bottom);
            }
        }
        let (to_deck, to_world) = match ride.matrix() {
            // One inverse per trail per frame, not one per edge.
            Some(a) => (Some(a.inverse()), Some(a)),
            None => (None, None),
        };

        // The **draw-set** gate: is this trail's MODEL in the frame's scene at all? A trail is one
        // of its model's emitters, and the reference ticks a model's emitters inside that model's
        // draw step — so a culled model's trail neither commits nor ages. FROZEN, the same shape
        // `simulate_particles` freezes a culled owner's pool in, resuming from frozen state with
        // one frame's dt rather than catching up. Until now the ribbon lane had no term here at
        // all: not distance, not the far-clip wall, not the frustum, not the exterior window, not
        // the portal PVS (each of which closed this hole for the
        // quad clouds and left it open for the strips).
        //
        // Only while the owner LIVES. Freezing a DRAINING trail strands it — it can never empty
        // its edges, so it never reaches the self-despawn above and leaks for the session; the
        // particle sim states the identical exclusion.
        let admitted = match (head, fade.as_ref()) {
            // A placed model — an ADT map doodad or a WMO prop. The full five-term rule, in its
            // one spelling, against the owner model's fade sphere.
            (Some(_), Some(f)) => f.in_draw_set(
                cam_pos,
                cam_fwd,
                farclip,
                // Lateral planes only (`intersect_far = false`): the depth bound is the far-clip
                // term inside `in_draw_set`, deliberately — see `view::within_farclip`.
                frustum.intersects_sphere(
                    &CullSphere {
                        center: f.center.into(),
                        radius: f.radius,
                    },
                    false,
                ),
                f.exterior_admitted(&exterior_gate, camera_instance),
                gates.room_admits(f),
            ),
            // ENTITY-owned — a creature, a GameObject, a missile, a held item. No fade sphere:
            // the population is bounded by server visibility, and by `alpha_src` below. Except
            // transports, which vmangos streams MAP-WIDE, so the bare wall applies to every
            // world-lane trail exactly as decision 0678 applies it to every world-lane emitter.
            // Measured live from the owner, never from a stored anchor: a frozen trail stops
            // refreshing its anchor, so gating on that would latch a moving owner out of
            // existence the first time it crossed the wall.
            (Some((node, _)), None) => {
                crate::view::within_farclip(farclip, cam_pos, cam_fwd, node, 0.0)
            }
            // Draining — never frozen (see above).
            (None, _) => true,
        };
        if !admitted {
            // Hold the clock with the trail. Edge expiry is measured against the SHARED clock
            // (`now - born`), so a freeze that let `now` run on would age every stored edge out
            // while the model was away and hand back an empty strip on re-entry — the opposite of
            // "resumes from frozen state". Advancing `born` by the frame's dt keeps each edge's
            // relative age exactly where the freeze found it.
            for e in edges.iter_mut() {
                e.born += dt;
            }
            continue;
        }

        // The keyed look tracks sample on the trail's clip clock (see [`RibbonTrail::age`]):
        // heights at edge-commit time (each edge keeps the width it was born with — the
        // reference stores the vertex pair per edge), colour/alpha per frame for the whole strip.
        *age += dt;
        let ms = *age * 1000.0;
        let h_above = def.height_above.sample_ms(ms).max(0.0);
        let h_below = def.height_below.sample_ms(ms).max(0.0);

        // The `+0xc0` **enable** gate, sampled LIVE against the sequence the host is playing. The
        // per-ribbon runtime byte `block+0xbc` IS the sampled `visibilityTrack`
        // value. Complete writer census — ctor `0x71b34c` = 0, loader default `0x70f80e` = 1,
        // then per frame `values[k0]` at `0x7176ee` (step arm) and `0x717714` (non-step arm; a u8
        // track is never blended, so both copy the same raw byte) inside `0x714260`. No
        // equipment/attach/sheathe writer exists anywhere. `0x718960` only READS it.
        //
        // Decision 1011 wired this and was right about the mechanism; decision 1013 unwound it and
        // was wrong. What actually made the trap look
        // wrong was gravity (see below) — the low rig sank instead of rising, so the tuft the
        // reference shows above the crown vanished and only the gated-off high rig had ever been
        // producing anything visible, as a downward column. Two faults, one screenshot.
        //
        // Clearing the byte KILLS THE WHOLE RIBBON'S DRAW — `0x7080c2` jumps to the collect loop's
        // continue at `0x708263`, emitting no record at all. The earlier "committed edges keep
        // fading" gloss is withdrawn by the same pass, so this gates the DRAW, not just the
        // commit: a trail whose gate clears vanishes on the frame it clears.
        let lit = def.visible.as_ref().is_none_or(|vis| match *seq {
            RibbonSeq::Fixed(a) => vis.at(a, 0.0),
            RibbonSeq::Host(h) => match hosts.get(h) {
                // Sequences exist: the playing one answers, and `playing_seq` already degrades to
                // the loader-idle clip. A slot with no clip row falls back to `Stand`(0).
                Ok((player, anims)) => {
                    let (anim, t) = crate::doodad_anim::playing_seq(player, anims)
                        .and_then(|(slot, t)| {
                            Some((anims.clips.iter().find(|c| c.seq_index == slot)?.anim_id, t))
                        })
                        .unwrap_or((0, 0.0));
                    vis.at(anim, t)
                }
                // No clock on the host at all — the instance is still being built this frame. The
                // reference never has this window (its loader seed arms a sequence the moment the
                // M2 goes LIVE, `0x70ebd0`), so answering "enabled" here would invent a state and
                // pop one frame of every gated trail. Hold dark until the host can answer.
                Err(_) => false,
            },
        });
        let head = lit.then_some(head).flatten();

        // Expire old edges (front = oldest), move the rest under gravity, commit new ones at rate.
        while edges
            .front()
            .is_some_and(|e| now - e.born >= def.edge_lifetime)
        {
            edges.pop_front();
        }
        // GRAVITY (`0x7b7e60`, the loop at `0x7b7fe7..0x7b807a`). Per frame, per live edge, into
        // BOTH vertices' world z:
        //
        //     term = gravity · ((age + age) + dt) · dt   ;   age += dt
        //
        // `(2a + dt)·dt` is identically `(a + dt)² − a²`, and the age advance follows immediately,
        // so the whole loop telescopes to a closed form — an edge sits at
        //
        //     z(t) = z_at_emit + gravity · t²        (t = seconds since commit, yards)
        //
        // with no ½ and no factor of 2. It is a **position** increment; the vertex is the only
        // state besides the per-edge age, there is no velocity, and it is scaled by nothing (the
        // interp factor the loop computes just above is never an operand here).
        //
        // And the sign is `fadd`, twice, with no `fchs` anywhere in the block: positions are world
        // space, WoW's +Z is UP, so a **positive gravity makes the trail RISE**. The corpus agrees
        // — of 590 ribbon records 86 are positive (totems 5.0, the cleanse family 0.5/1.0) and 16
        // negative, including ±0.5 up/down PAIRS inside one model, which a single-signed reading
        // cannot explain at all. Our old `pos.y -= 2·g·dt` was wrong twice over: constant velocity
        // instead of `g·t²`, and falling instead of rising. On the Frost Trap that turned a
        // 0.6–1.3 yd tuft rising off the crown into a 2–3 yd column smeared to the ground.
        // The gravity axis is world up in EITHER store: `A` is `translate·Rz` (yaw only), and a
        // yaw leaves `+Y` alone — so a trail sags/rises the same way on a deck as on the ground,
        // with no fold. That invariance is why [`crate::ride_frame::ride_matrix`] drops
        // pitch/roll rather than carrying the transport's full rotation.
        for e in edges.iter_mut() {
            let term = gravity_step(def.gravity, e.age, dt);
            e.top.y += term;
            e.bottom.y += term;
            e.age += dt;
        }
        // EMISSION — `edgesPerSecond` is a true rate, not a per-frame cadence: the reference
        // commits `n = floor(dt·eps + phase)` edges this frame, carries the fraction as the phase,
        // and **backdates** each one inside the frame (`0x7b7f60`). A one-edge-per-frame cap
        // silently thins every trail below `eps` frames per second — the whole trail, at 30 fps
        // with the Frost Trap's `eps` 30, is half the edges it should hold.
        if let Some((node, axis)) = head {
            // BIRTH fold: the node and its cross-section axis are live WORLD values (the owner's
            // bone matrix); an edge is committed in the store's frame. Off a transport this is the
            // identity, which is the whole world.
            let (node, axis) = match to_deck {
                Some(inv) => (inv.transform_point3(node), inv.transform_vector3(axis)),
                None => (node, axis),
            };
            *accumulator += def.edges_per_second * dt;
            let n = accumulator.floor().max(0.0);
            *accumulator -= n;
            // Sub-frame backdating: the n-th edge of this frame was emitted `k/n · dt` ago (the
            // node itself only has this frame's position, so the sample point is shared — the
            // backdate is what the age, and therefore the gravity rise, is measured from).
            let n = (n as usize).min(MAX_EDGES.saturating_sub(edges.len()));
            for k in 0..n {
                let back = dt * (n - 1 - k) as f32 / n as f32;
                edges.push_back(Edge {
                    top: node + axis * h_above,
                    bottom: node - axis * h_below,
                    born: now - back,
                    age: back,
                });
            }
        }

        // Write the strip into the shared stream: live head first (while the owner lives), then
        // committed edges newest→oldest. u slides with age across the tex-slot cell (the
        // texture's transparent tail is the fade); v spans the cell band. An idle trail — no
        // strip yet, or a non-resident texture — pushes nothing and commits nothing: the old
        // "don't rewrite an already-empty mesh" guard is now the structure itself.
        if !images.contains(&*texture) {
            continue;
        }
        // The gate again, on the DRAW: `0x7080c2` skips the whole record when the byte is 0, so a
        // gated-off ribbon shows nothing — not a fading remainder. The sim above still ran (edges
        // age and expire exactly as `0x7b7e60` ages them), which is what makes the trail resume
        // mid-strip rather than from empty when the byte comes back.
        if !lit {
            continue;
        }
        // An invisible MODEL has no streamer: the reference's ribbon render leg reads the owning
        // model's render alpha and drops the draw below a threshold. This is what
        // takes your own weapon's enchant trail out of your face in first person, and keeps a
        // not-yet-shown unit's trail off the screen while its body is still at alpha 0.
        if alpha_src.is_some_and(|e| model_alphas.get(e) <= 1e-3) {
            continue;
        }
        let n = edges.len() + usize::from(head.is_some());
        if n < 2 {
            continue;
        }
        let (rows, cols) = (def.tile_rows.max(1), def.tile_cols.max(1));
        let cell = def.tex_slot.min(rows * cols - 1);
        let (u0, u1) = (
            f32::from(cell % cols) / f32::from(cols),
            f32::from(cell % cols + 1) / f32::from(cols),
        );
        let (v0, v1) = (
            f32::from(cell / cols) / f32::from(rows),
            f32::from(cell / cols + 1) / f32::from(rows),
        );
        // RAW authored RGB — the gamma decode happens once in the effect shader,
        // covering the texture term too. Alpha is a blend weight, raw.
        let rgb = def.color.sample_ms(ms);
        let rgba = [rgb[0], rgb[1], rgb[2], def.alpha.sample_ms(ms).max(0.0)];
        // The trail's SORT anchor — the live head node (the point the material path's entity
        // translation used to carry; same sort-tie flashing fix as the particle clouds).
        // Draining trails anchor on their newest surviving edge.
        // DRAW fold: stored → world (`0xcf5b68 = A · T · S`). The live head is already world —
        // it was never committed.
        let out = |p: Vec3| to_world.map_or(p, |a| a.transform_point3(p));
        let anchor = head.map(|(node, _)| node).unwrap_or_else(|| {
            let e = edges.back().expect("n >= 2 ⇒ edges exist while draining");
            out((e.top + e.bottom) * 0.5)
        });
        entity_tf.translation = anchor;
        // Post-propagation frame: publish directly (see the particle sim's matching note; trail
        // entities live at the world root, the direct write is exact).
        *entity_global = GlobalTransform::from(*entity_tf);
        // The edge sequence, head first then newest→oldest — each consecutive pair becomes one
        // quad whose corner order reproduces the old strip's exact triangles: strip triangles
        // (t₀,b₀,t₁),(b₀,b₁,t₁) = quad [b₀,b₁,t₁,t₀] under the lane's [0,1,2, 0,2,3] pattern.
        let mut pairs: Vec<(Vec3, Vec3, f32)> = Vec::with_capacity(n);
        if let Some((node, axis)) = head {
            pairs.push((node + axis * h_above, node - axis * h_below, 0.0));
        }
        for e in edges.iter().rev() {
            pairs.push((
                out(e.top),
                out(e.bottom),
                ((now - e.born) / def.edge_lifetime).clamp(0.0, 1.0),
            ));
        }
        verdict.drawn += 1;
        let start = quads.begin();
        for w in pairs.windows(2) {
            let ((t0, b0, a0), (t1, b1, a1)) = (w[0], w[1]);
            let (ua0, ua1) = (u0 + (u1 - u0) * a0, u0 + (u1 - u0) * a1);
            for (pos, uv) in [
                (b0, [ua0, v1]),
                (b1, [ua1, v1]),
                (t1, [ua1, v0]),
                (t0, [ua0, v0]),
            ] {
                quads.verts.push(EffectVertex {
                    pos: pos.to_array(),
                    uv,
                    color: rgba,
                });
            }
        }
        quads.commit_quads(
            start,
            EffectDrawSpec {
                cam,
                texture: texture.id(),
                blend: def.blend.into(),
                // params.x = the per-blend fog-colour policy (the M2 batch state setter's
                // table, `0x70baf0` — ribbons ride the same trio): additive
                // trails fog toward BLACK, fading under the storm veil instead of adding grey;
                // alpha/opaque trails fog toward the scene colour. (No ribbon authors the
                // particle "unfogged" file flag — pass 0.)
                fog: EffectFog::for_blend(0, def.blend),
                // Ribbons keep the lane's unlit default: the M2 ribbon record has no flag word
                // to read the particle path's unlit bit off, and the trail corpus is additive
                // weapon/spell art authored to burn at its own colour. Revisit only with a
                // byte law for the ribbon batch state, not by analogy with particles.
                lighting: crate::particles::buffer::EffectLighting::None,
                anchor,
                // The owner rung, dropped under the water pass when the MODEL sits on the eye's
                // far side of its water plane — the model's bound centre with the bound-radius
                // slack, never the whipping head node (the ribbon leg reads the model's
                // side-A boolean verbatim, `0x7081f1`). The model frame is `alpha_src` — "the
                // MODEL INSTANCE" — with the owner (possibly a joint) seconding as the walk
                // seed; an unresolvable matrix falls back to the sign test at the head.
                bias: *bias
                    + if crate::particles::model_far_side(
                        &interleave,
                        alpha_src.or(*owner),
                        alpha_src.and_then(|e| transforms.get(e).ok()),
                        *water_bound,
                        anchor,
                    ) {
                        crate::sky_order::FAR_SIDE_BIAS
                    } else {
                        0.0
                    },
                raster_bias: 0,
                raster_slope: 0.0,
                cam_relative: false,
                no_depth_test: false,
                main_entity: entity,
                light: None, // trails never carry a light override (world lane only)
                clip: None,  // …and never ride a UI model pane's atlas cell
            },
        );
    }
}

/// Registers the per-frame ribbon simulation. Trails are spawned by the model spawn sites
/// (creatures, held items, missiles, spell effects, doodads) via [`spawn_ribbon`].
pub struct RibbonPlugin;

impl Plugin for RibbonPlugin {
    fn build(&self, app: &mut App) {
        // PostUpdate, after the billboard joint palette — same law and reason as the particle
        // sim: a trail node on a billboarded/animated bone must sample the frame the palette
        // just wrote (see `billboard_joint_palette`'s consumer note).
        app.init_resource::<RibbonVerdict>().add_systems(
            PostUpdate,
            simulate_ribbons
                .in_set(crate::billboard::BillboardPlace)
                .after(crate::billboard::billboard_joint_palette)
                .after(crate::rig_anim::finalize_rig_worlds),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::gravity_step;

    /// The per-frame gravity term telescopes to the closed form the bytes imply: stepping
    /// `gravity · ((2·age) + dt) · dt` and advancing `age` by `dt` leaves an edge exactly
    /// `gravity · t²` from where it was emitted, **whatever the frame rate** — which is the
    /// property that makes a trail look the same at 30 and 144 fps. Sign included: positive
    /// gravity rises.
    #[test]
    fn gravity_telescopes_to_g_t_squared_at_any_frame_rate() {
        for &g in &[0.5_f32, 1.0, 1.5, 2.0, 5.0, -1.0] {
            for &dt in &[1.0 / 144.0_f32, 1.0 / 60.0, 1.0 / 30.0, 1.0 / 15.0] {
                let steps = (1.0 / dt).round() as usize; // exactly one second of frames
                let (mut z, mut age) = (0.0_f32, 0.0_f32);
                for _ in 0..steps {
                    z += gravity_step(g, age, dt);
                    age += dt;
                }
                let t = steps as f32 * dt;
                assert!(
                    (z - g * t * t).abs() < 1e-4,
                    "g {g} at dt {dt}: got {z}, want {}",
                    g * t * t
                );
            }
        }
        // The sign is the half that was inverted: a positive gravity RISES.
        assert!(gravity_step(2.0, 0.0, 0.016) > 0.0);
        assert!(gravity_step(-1.0, 0.0, 0.016) < 0.0);
    }

    /// The Frost Trap's four low streamers, with their authored `gravity` and `edgeLifetime`:
    /// each rises `g·L²` = 0.605 / 1.000 / 1.215 / 1.280 yd over an edge's life, from a node at
    /// model z 0.129. That is the compact tuft standing off a crown whose own geometry stops at
    /// z 0.637 — and it is what the old constant-velocity fall (1.1–3.2 yd DOWNWARD from the
    /// twelve upper streamers at z 1.55) turned into a column reaching the ground.
    #[test]
    fn frost_trap_low_rig_rises_into_a_tuft_over_the_crown() {
        for (g, life, want) in [
            (0.5_f32, 1.1_f32, 0.605_f32),
            (1.0, 1.0, 1.000),
            (1.5, 0.9, 1.215),
            (2.0, 0.8, 1.280),
        ] {
            let dt = 1.0 / 60.0;
            let steps = (life / dt).round() as usize;
            let (mut z, mut age) = (0.0_f32, 0.0_f32);
            for _ in 0..steps {
                z += gravity_step(g, age, dt);
                age += dt;
            }
            assert!((z - want).abs() < 0.01, "g {g} life {life}: {z} vs {want}");
            assert!(0.129 + z > 0.637, "clears the crown's own geometry");
        }
    }
}
