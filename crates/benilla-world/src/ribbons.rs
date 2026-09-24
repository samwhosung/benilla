//! M2 ribbon trails: weapon enchant trails, wisp streamers, spell-missile trails. The 1.12 client
//! keeps a ribbon as a ring of edges, vertex pairs across the node the live bone matrix places
//! each frame, committed at `edgesPerSecond`, aged out at `edgeLifetime` and drawn as a strip whose
//! `u` slides with age, so the texture's transparent tail is the fade. When its owner goes, a
//! trail drains and despawns itself.

use std::collections::VecDeque;

use benilla_assets::coords::wow_to_bevy;
use benilla_assets::ModelRibbon;
use benilla_formats::ParticleBlend;
use bevy::camera::primitives::{Frustum, Sphere as CullSphere};
use bevy::camera::Projection;
use bevy::prelude::*;

use crate::particles::buffer::{EffectDrawSpec, EffectFog, EffectQuads, EffectVertex};
use crate::view::WorldCamera;

/// A backstop cap on stored edges; the reference's ring holds `ceil(rate·lifetime) + 2`.
const MAX_EDGES: usize = 512;

/// This frame's ribbon census: a trail carries no `ModelPart` or `ParticleEmitter`, so neither
/// the visibility nor the particle census counts it.
#[derive(Resource, Default)]
pub struct RibbonVerdict {
    /// Trails alive this frame, streaming or draining.
    pub trails: usize,
    /// Trails that wrote a strip into the shared effect stream.
    pub drawn: usize,
}

/// One committed edge: its vertex pair in the stored frame and its birth on the shared clock.
struct Edge {
    top: Vec3,
    bottom: Vec3,
    born: f32,
    /// Seconds alive from the backdated birth: the reference's per-edge age array
    /// (`emitter+0x0c`), which the gravity step reads.
    age: f32,
}

/// One frame's gravity rise for an edge, world +Z yards: the reference's
/// `gravity · ((age + age) + dt) · dt` (`0x7b7e60`, `0x7b8007`..`0x7b800f`), then `age += dt`. It
/// telescopes to `gravity · t²`, upward for a positive gravity (two `fadd`s, no `fchs`).
fn gravity_step(gravity: f32, age: f32, dt: f32) -> f32 {
    gravity * ((age + age) + dt) * dt
}

/// A live ribbon trail riding `owner`, drawn in world space; its translation is the sort anchor.
#[derive(Component)]
pub struct RibbonTrail {
    def: benilla_formats::RibbonEmitterDef,
    /// The emission origin in the owner's frame (`position − bone_pivot` for a joint owner).
    local_offset: Vec3,
    /// The node source; `None` once the owner is gone and the trail drains. The reference frees a
    /// model's emitters at its dtor (`0x70e313`) and keeps an ending effect's model while its
    /// particles drain (`HasLiveParticles` `0x7b5f60`); whether its ribbons drain too is untraced.
    owner: Option<Entity>,
    /// The clock the `+0xc0` enable gate is sampled against every frame.
    seq: RibbonSeq,
    /// The model whose [`crate::model_fade::ModelAlpha`] gates the draw (`None` always draws): the
    /// reference's ribbon leg drops the draw below a render-alpha threshold (`0x707680`, alpha
    /// `block+0x3c × Model+0x19c`). Whether that alpha also scales the strip is untraced.
    alpha_src: Option<Entity>,
    /// The placed model's [`crate::particles::EmitterFade`]: the reference ticks and draws a
    /// model's emitters inside that model's draw step, so a trail takes its quad clouds' draw-set
    /// gate. Held by value: as a component it would join the particle sim's fade query and, by its
    /// `WmoGroupVis`, `apply_model_visibility`'s. `None` for an entity-owned trail.
    fade: Option<crate::particles::EmitterFade>,
    /// The frame the committed edges are stored in: world on the ground, the deck while the owning
    /// model rides a transport ([`crate::ride_frame`]). The reference stores and re-projects them
    /// (birth `0x718dd8` → `0x7b76c0`, draw `0x70d87d` → `0x7b80c0`) with no `0x100` gate: a
    /// trail is always ride-framed, a particle cloud only in world mode.
    ride: crate::ride_frame::StoredFrame,
    /// Committed edges, newest last; the live head is added only at draw.
    edges: VecDeque<Edge>,
    accumulator: f32,
    /// Seconds since spawn: the clip clock the keyed tracks sample, as an effect's ribbons spawn at
    /// its clip start (a persistent trail's tracks are constant).
    age: f32,
    texture: Handle<Image>,
    /// The owner-last draw-order rung its model's quad clouds take, added to the sort key.
    bias: f32,
    /// The owner model's bound sphere: the water-plane side is the model's, never the head's.
    water_bound: (Vec3, f32),
}

impl RibbonTrail {
    /// The emitter bone, the id `WOW_PHASE=particles:<bone>` arms on and the asset dumps print.
    pub fn bone(&self) -> u16 {
        self.def.bone
    }

    /// The committed strip in world space, as drawn; the trail census reads it.
    pub fn strip_world(&self) -> impl Iterator<Item = Vec3> + '_ {
        self.edges
            .iter()
            .flat_map(move |e| [self.ride.to_world(e.top), self.ride.to_world(e.bottom)])
    }

    /// The transport this trail's edges are stored against; `None` on the ground.
    pub fn deck(&self) -> Option<Entity> {
        self.ride.source()
    }

    /// The authored edge lifetime (s); a streak's world length over it gives the host's speed.
    pub fn edge_lifetime(&self) -> f32 {
        self.def.edge_lifetime
    }

    /// The authored blend and the committed edge count (0 = nothing drawn yet).
    pub fn shape(&self) -> (ParticleBlend, usize) {
        (self.def.blend, self.edges.len())
    }
}

/// What decides a trail's `+0xc0` enable gate: the reference's per-ribbon `block+0xbc` byte,
/// re-read every frame (`0x717660`).
#[derive(Clone, Copy)]
pub enum RibbonSeq {
    /// The sequence this entity plays, re-read each frame, since a trap springs or a door opens.
    Host(Entity),
    /// A fixed `AnimationData.dbc` id for life, such as a worn item's `Stand` (0).
    Fixed(u16),
}

/// Spawns a trail for one [`ModelRibbon`] riding `owner`: a host-bone joint (`use_pivot` rebases
/// by the def's baked pivot) or a model or item root. `owner_scale`, the placement's largest scale,
/// takes [`ModelRibbon::owner_reach`] to world yards for the draw-order rung. `None` without a
/// texture or with degenerate emission. A gated trail still spawns: its gate is sampled live.
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
    // `WOW_NO_PARTICLES` turns ribbons off too.
    if std::env::var_os("WOW_NO_PARTICLES").is_some() {
        return None;
    }
    let texture = ribbon.texture.clone()?;
    let def = ribbon.def.clone();
    // The peaks, not the first keys: a keyed slash (HolySmite) is born at height 0.
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
                // The sim writes the sort anchor here each frame, where the phase probe reads it.
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
                    // A model's emitters draw after its batches: the quad clouds' rung and reach.
                    bias: crate::particles::owner_last_bias(ribbon.owner_reach * owner_scale),
                    water_bound: ribbon.water_bound,
                },
            ))
            .id(),
    )
}

/// Per frame: places nodes, commits and expires edges, applies gravity and writes the strips.
pub(crate) fn simulate_ribbons(
    time: Res<Time>,
    mut commands: Commands,
    // Owners only, disjoint from the trail query's `&mut GlobalTransform`.
    transforms: Query<&GlobalTransform, Without<RibbonTrail>>,
    // The enable gate's clock for a `RibbonSeq::Host`.
    hosts: Query<(&AnimationPlayer, &benilla_assets::ModelAnimations)>,
    images: Res<Assets<Image>>,
    mut quads: ResMut<EffectQuads>,
    model_alphas: crate::model_fade::ModelAlphas,
    // World lane only: a booth-parked owner's strip is cut by the shader's far-clip wall.
    world_cam: Query<(Entity, &GlobalTransform, &Frustum, &Projection), With<WorldCamera>>,
    interleave: crate::particles::WaterInterleave,
    // The draw-set gate's scene inputs, the same bundle `simulate_particles` reads.
    gates: crate::particles::sim::SceneGates,
    rides: crate::ride_frame::RideFrames,
    mut verdict: ResMut<RibbonVerdict>,
    mut trails: Query<
        (
            Entity,
            &mut RibbonTrail,
            &mut Transform,
            &mut GlobalTransform,
        ),
        // Keeps the camera's `&GlobalTransform` read disjoint from these writes.
        Without<WorldCamera>,
    >,
) {
    *verdict = RibbonVerdict::default();
    let Ok((cam, cam_tf, frustum, projection)) = world_cam.single() else {
        return;
    };
    let cam_pos = cam_tf.translation();
    // The owner mesh's far-clip axis, so a trail crosses the wall with its model.
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

        if owner.is_some_and(|o| !transforms.contains(o)) {
            *owner = None;
        }
        let head = owner.and_then(|o| transforms.get(o).ok()).map(|owner_gt| {
            let node = owner_gt.transform_point(*local_offset);
            // The cross-section axis is the live bone's local +Y: `0x7b76c0` captures the basis
            // each frame, and `0x7b6990` spans `±heightAbove/Below` along that row alone.
            let axis = (owner_gt.rotation() * wow_to_bevy([0.0, 1.0, 0.0])).normalize_or(Vec3::Y);
            (node, axis)
        });
        if head.is_none() && edges.is_empty() {
            commands.entity(entity).despawn();
            continue;
        }
        verdict.trails += 1;

        // The owning model's transport, up the `ParentModel` chain; one streamed out reads as
        // none, which hands the edges back to the world where they stood.
        let deck = alpha_src
            .or(*owner)
            .and_then(|model| rides.source(model))
            .and_then(|t| {
                transforms
                    .get(t)
                    .ok()
                    .map(|gt| (t, crate::ride_frame::ride_matrix(gt)))
            });
        // Boarding and leaving re-express the stored edges (the reference's `0x7187f0` →
        // `0x7b7bc0`); a moving deck folds nothing, since the draw's live `A` carries the strip.
        if let Some(fold) = ride.retarget(deck) {
            for e in edges.iter_mut() {
                e.top = fold.transform_point3(e.top);
                e.bottom = fold.transform_point3(e.bottom);
            }
        }
        let (to_deck, to_world) = match ride.matrix() {
            Some(a) => (Some(a.inverse()), Some(a)),
            None => (None, None),
        };

        // The draw-set gate: the reference ticks a model's emitters inside its draw step, so a
        // culled model's trail freezes, resuming with one frame's dt. A draining trail never
        // freezes: frozen, it could never empty and despawn.
        let admitted = match (head, fade.as_ref()) {
            (Some(_), Some(f)) => f.in_draw_set(
                cam_pos,
                cam_fwd,
                farclip,
                // Lateral planes only: the depth bound is `in_draw_set`'s far-clip term.
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
            // Entity-owned: server visibility bounds these, but vmangos streams transports
            // map-wide, so the far-clip wall applies as to every world-lane emitter. Measured from
            // the live node: a frozen trail's stale anchor would latch a moving owner out.
            (Some((node, _)), None) => {
                crate::view::within_farclip(farclip, cam_pos, cam_fwd, node, 0.0)
            }
            // Draining: never frozen.
            (None, _) => true,
        };
        if !admitted {
            // Expiry reads the shared clock (`now - born`), so a frozen trail moves `born` on by
            // dt to hold each edge's age.
            for e in edges.iter_mut() {
                e.born += dt;
            }
            continue;
        }

        // Heights sample at commit, since the reference stores each edge's vertex pair; colour
        // and alpha sample per frame.
        *age += dt;
        let ms = *age * 1000.0;
        let h_above = def.height_above.sample_ms(ms).max(0.0);
        let h_below = def.height_below.sample_ms(ms).max(0.0);

        // The `+0xc0` enable gate, sampled live against the host's sequence. The per-ribbon byte
        // `block+0xbc` is the sampled `visibilityTrack`: 0 from the ctor (`0x71b34c`), 1 from the
        // loader (`0x70f80e`), then each frame `values[k0]` in `0x714260` (`0x7176ee` step arm,
        // `0x717714` non-step arm); nothing else writes it and `0x718960` only reads it. A clear
        // byte skips the ribbon's whole draw (`0x7080c2` jumps to `0x708263`), not just commits.
        let lit = def.visible.as_ref().is_none_or(|vis| match *seq {
            RibbonSeq::Fixed(a) => vis.at(a, 0.0),
            RibbonSeq::Host(h) => match hosts.get(h) {
                // The playing sequence; a slot with no clip row reads as `Stand` (0).
                Ok((player, anims)) => {
                    let (anim, t) = crate::doodad_anim::playing_seq(player, anims)
                        .and_then(|(slot, t)| {
                            Some((anims.clips.iter().find(|c| c.seq_index == slot)?.anim_id, t))
                        })
                        .unwrap_or((0, 0.0));
                    vis.at(anim, t)
                }
                // No clock yet: the host is still being built. The reference arms a sequence as the
                // M2 goes live (`0x70ebd0`), so this holds dark rather than invent a lit frame.
                Err(_) => false,
            },
        });
        let head = lit.then_some(head).flatten();

        while edges
            .front()
            .is_some_and(|e| now - e.born >= def.edge_lifetime)
        {
            edges.pop_front();
        }
        // Gravity (`0x7b7e60`, loop `0x7b7fe7..0x7b807a`): both vertices of every live edge rise
        // by `gravity_step`, with no velocity and no other scale. The axis is world up in either
        // store, which is why `crate::ride_frame::ride_matrix` is yaw only.
        for e in edges.iter_mut() {
            let term = gravity_step(def.gravity, e.age, dt);
            e.top.y += term;
            e.bottom.y += term;
            e.age += dt;
        }
        // Emission: the reference commits `floor(dt·eps + phase)` edges a frame, carries the
        // fraction as the phase and backdates each inside the frame (`0x7b7f60`).
        if let Some((node, axis)) = head {
            // Birth fold: the live world node and axis into the store's frame.
            let (node, axis) = match to_deck {
                Some(inv) => (inv.transform_point3(node), inv.transform_vector3(axis)),
                None => (node, axis),
            };
            *accumulator += def.edges_per_second * dt;
            let n = accumulator.floor().max(0.0);
            *accumulator -= n;
            // The edges share this frame's node; each ages, and rises, from its backdated birth.
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

        // The strip, head first: `u` slides with age across the tex-slot cell, `v` spans its band.
        if !images.contains(&*texture) {
            continue;
        }
        // The gate on the draw (`0x7080c2`): a gated-off ribbon shows nothing, while its edges
        // still age as `0x7b7e60` ages them, so it resumes mid-strip.
        if !lit {
            continue;
        }
        // An invisible model draws no streamer: a first-person weapon's, a unit's not yet shown.
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
        // Raw authored RGB and alpha: the effect shader decodes gamma once, texture included.
        let rgb = def.color.sample_ms(ms);
        let rgba = [rgb[0], rgb[1], rgb[2], def.alpha.sample_ms(ms).max(0.0)];
        // The sort anchor: the live head, or a draining trail's newest edge. The draw fold maps
        // stored to world (`0xcf5b68 = A · T · S`); the head is already world.
        let out = |p: Vec3| to_world.map_or(p, |a| a.transform_point3(p));
        let anchor = head.map(|(node, _)| node).unwrap_or_else(|| {
            let e = edges.back().expect("n >= 2 ⇒ edges exist while draining");
            out((e.top + e.bottom) * 0.5)
        });
        entity_tf.translation = anchor;
        // After propagation, so publish directly; trail entities sit at the world root.
        *entity_global = GlobalTransform::from(*entity_tf);
        // Each consecutive pair is one quad `[b₀, b₁, t₁, t₀]`, which the lane's `[0,1,2, 0,2,3]`
        // splits into the strip's triangles `(t₀, b₀, t₁)` and `(b₀, b₁, t₁)`.
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
                // The M2 batch state's per-blend fog colour (`0x70baf0`): additive trails fog
                // toward black, the rest toward the scene colour. No ribbon has the unfogged flag.
                fog: EffectFog::for_blend(0, def.blend),
                // Unlit: the ribbon record has no flag word for the particles' unlit bit, and the
                // reference's ribbon batch lighting is untraced.
                lighting: crate::particles::buffer::EffectLighting::None,
                anchor,
                // The owner rung, dropped under the water pass when the model's bound, radius
                // slack included, is on the eye's far side of its water plane (`0x7081f1`); with
                // no model matrix, the head's side decides.
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

/// Registers the per-frame ribbon simulation; the model spawn sites call [`spawn_ribbon`].
pub struct RibbonPlugin;

impl Plugin for RibbonPlugin {
    fn build(&self, app: &mut App) {
        // After the billboard joint palette and the rig worlds: a node on a billboarded or
        // animated bone samples the palette this frame wrote.
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
        // A positive gravity rises.
        assert!(gravity_step(2.0, 0.0, 0.016) > 0.0);
        assert!(gravity_step(-1.0, 0.0, 0.016) < 0.0);
    }

    /// The Frost Trap's four low streamers (authored `gravity`, `edgeLifetime`) rise `g·L²` over an
    /// edge's life from a node at model z 0.129, clearing the crown's own geometry at z 0.637.
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
