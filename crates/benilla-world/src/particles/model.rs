//! **3-D MODEL particles**: an emitter whose
//! record names a geometry model renders each live particle as a tiny 3-D instance of that
//! model — quaternion-oriented, tumbling, over-life scaled and tinted — instead of a billboard
//! quad (Whirlwind's blades, Cone of Cold's shards, the cyclones, Death Wish). The sim
//! integrates position + spin ([`super::sim`]); this module owns the DRAW side: a per-emitter
//! pool of instance entities grown on demand, positioned after the sim each frame.
//!
//! Draw law (`0x7b4840` → `0x7b4510`): per particle, the instance transform is the particle
//! quaternion (× the emitter frame) at the particle position, scaled by the over-life SIZE
//! ramp; over-life COLOR rides a per-instance tint clone (`WowModelExt::tint`, the fx-tint
//! mechanism) and over-life ALPHA rides the per-instance `MeshTag` alpha field. The
//! reference's optional per-emitter depth sort (rt+0x1ac & 0x10) is folded into the pool-order
//! simplification named in decision 0148. The reference draws each instance through its
//! generic model-render pass; our instances are the geometry model's submeshes with their own
//! materials — mini-model *animation* (a rigged geometry model) is not run: every spell-corpus
//! geometry target probed is a static mesh, and a rigged one would surface as a visibly stiff
//! instance, the recorded trigger to extend this.

use benilla_assets::coords::wow_to_bevy;
use benilla_assets::M2Model;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::mesh::MeshTag;
use bevy::prelude::*;

use crate::model_render::{model_material, MaterialCache, ShadeSel};
use benilla_assets::materials::WowModelMaterial;

use super::{ChildDraw, ParticleEmitter};

/// A hard cap on one emitter's instance pool — far above any authored steady state
/// (rate ≤ ~25/s × life ≤ 3 s across the corpus); particles past it simply aren't drawn.
const MAX_INSTANCES: usize = 128;

/// One pooled instance slot: the geometry model's submeshes as world-root [`ChildDraw`]
/// entities (flat — no hierarchy, so the sim-frame transform write is exact), each with its
/// per-instance tint-clone material.
pub(super) struct ModelInstance {
    pub(super) meshes: Vec<(Entity, Handle<WowModelMaterial>)>,
}

/// Grow + position each model-particle emitter's instance pool from its freshly-simulated
/// pool. Runs after [`super::sim::simulate_particles`] in the same set, so instances land on
/// this frame's positions (the anchored-exactness rule).
pub(super) fn update_model_particles(
    mut commands: Commands,
    models: Res<Assets<M2Model>>,
    mut forms: ResMut<crate::model_forms::ModelForms>,
    mut mesh_assets: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<WowModelMaterial>>,
    light: Res<crate::lighting::SharedLightBuffer>,
    mut emitters: Query<&mut ParticleEmitter>,
    mut draws: Query<
        (
            &mut Transform,
            &mut GlobalTransform,
            &mut Visibility,
            &mut MeshTag,
        ),
        With<ChildDraw>,
    >,
) {
    for mut emitter in &mut emitters {
        let Some(geometry) = emitter.geometry.clone() else {
            continue;
        };
        // A GATED emitter's pool is frozen and the sim already edge-hid its instances
        // (decision 1480): running this body anyway re-showed them the same frame —
        // `Visibility::Inherited` below undid the hide, so a frozen emitter's shards drew
        // forever — and kept every instance's Transform/GlobalTransform/MeshTag/material a
        // per-frame write. The sim clears `gated` before this system in the same chained set,
        // so a thawed pool resumes the frame it wakes.
        if emitter.gated {
            continue;
        }
        let Some(model) = models.get(&geometry) else {
            continue; // still loading — particles simulate meanwhile, nothing draws yet
        };
        // The geometry model's render forms, built NOW rather than paced (decision 0834): a
        // spell's visual must not lag its cast, and a particle-geometry model is a handful of
        // tiny batches — the booth-lane exception, not the streaming rule.
        forms.ensure_now_static(&geometry, &model.submeshes, &mut mesh_assets);
        // Grow the pool to the live count (bounded).
        let rung = emitter.owner_rung();
        let want = emitter.particles.len().min(MAX_INSTANCES);
        while emitter.model_instances.len() < want {
            let stat_forms = forms.slices(&geometry).stat;
            let meshes = model
                .submeshes
                .iter()
                .enumerate()
                .map(|(pi, sub)| {
                    // A fresh (never-deduped) material per instance — its `tint` is mutated
                    // every frame by the over-life ramp, so instances must not share.
                    let mut throwaway = MaterialCache::default();
                    let material = model_material(
                        &mut throwaway,
                        &mut materials,
                        sub.texture.clone(),
                        sub.blend,
                        sub.two_sided,
                        false,
                        false,
                        sub.emissive,
                        sub.additive,
                        false,
                        sub.no_depth_write,
                        sub.no_depth_test,
                        sub.fog_policy,
                        sub.env_map, // texture_unit_lookup > 2 ⇒ the runtime generates this batch's UVs
                        // The same LIT lane as every entity M2 (`0x69e280`).
                        ShadeSel::Lit,
                        0,
                        None,
                        None, // over-life tint owns the channel — never the authored M2Color loop
                        None,
                        None,
                        false,
                        false, // an effect model is never a skybox
                        &light.0,
                        None, // a shard's material is shared by the whole emitter
                    );
                    // Realized at once: the over-life ramp writes this material through
                    // `get_mut` every frame, from the first (`model_render::lazy`).
                    crate::model_render::lazy::realize(&mut materials, material.id());
                    // The owner-last draw-order rung, stamped on after the fact: a 3-D model
                    // particle is one of its owner's emitters exactly like the quad cloud beside
                    // it, and the reference draws them in one bracket after that model's batches
                    // (`ParticleEmitter::owner_rung`). It goes here rather than through
                    // `model_material`, which is the shared M2-batch recipe: every batch in the
                    // world would have to carry a particle-only argument and a cache-key field to
                    // move one number. These instance materials are per-instance throwaways
                    // (their `tint` is already mutated every frame), so nothing else sees it.
                    //
                    // BUCKETED, and transparent-pass only (decision 0945). On a material the rung
                    // is also a *pipeline-key axis* (bevy folds `depth_bias as i32` into the key
                    // — 0837's law), and 32 integer rungs × every blend state is an open key
                    // space no warm pass can pre-compile: each first-seen combination was a
                    // synchronous render-thread pipeline compile mid-spell-cast.
                    // `owner_last_rung_bucket` snaps UP (0721's blessed direction) to the closed
                    // set `pipe_warm`'s menagerie compiles behind the loading cover; the bucket
                    // is ≥ the exact rung, so a shard never sorts under its sibling quad cloud
                    // (which keeps the exact value — its rung is queue-side, never a material).
                    // Opaque/mask shard batches take no rung at all: sort bias is a
                    // transparent-pass concept, and on an opaque material it would only mint an
                    // unwarmable pipeline variant.
                    if let Some(m) = materials.get_mut(&material) {
                        if m.base.alpha_mode == AlphaMode::Blend {
                            m.base.depth_bias = benilla_formats::owner_last_rung_bucket(rung);
                        }
                    }
                    let entity = commands
                        .spawn((
                            Mesh3d(
                                stat_forms
                                    .get(pi)
                                    .map(|(h, _)| h.clone())
                                    .unwrap_or_default(),
                            ),
                            MeshMaterial3d(material.clone()),
                            Transform::IDENTITY,
                            Visibility::Hidden,
                            NoFrustumCulling,
                            MeshTag(crate::mesh_tag::alpha_bits(1.0)),
                            ChildDraw,
                        ))
                        .id();
                    (entity, material)
                })
                .collect();
            emitter.model_instances.push(ModelInstance { meshes });
        }
        // Position live slots; hide the rest.
        let anchored = !emitter.def.model_space();
        let inst_scale = if emitter.def.scale_size_by_instance() {
            emitter.placement.scale.x.max(1e-4)
        } else {
            1.0
        };
        for (i, slot) in emitter.model_instances.iter().enumerate() {
            let Some(p) = emitter.particles.get(i) else {
                for (e, _) in &slot.meshes {
                    if let Ok((_, _, mut vis, _)) = draws.get_mut(*e) {
                        if *vis != Visibility::Hidden {
                            *vis = Visibility::Hidden;
                        }
                    }
                }
                continue;
            };
            let u = (p.age / p.life).clamp(0.0, 1.0);
            // 3-D model particles carry no texture atlas — only colour and size reach them.
            let ol = emitter.def.over_life.sample(u);
            let (mut rgba, size) = (ol.color, ol.size);
            // The owning model's render alpha, same fold as the quad lane (decision 0827) — here
            // into the instance's `MeshTag` alpha, which is where a 3-D particle carries it.
            rgba[3] *= emitter.render_alpha();
            let tf = if anchored {
                Transform {
                    // World mode: the instance transform IS the stored one (decision 1585) —
                    // through the ride frame, which is the identity off a transport (1591).
                    translation: emitter.ride.to_world(p.pos),
                    rotation: emitter.ride.rotation() * p.quat,
                    scale: Vec3::splat(size * inst_scale),
                }
            } else {
                Transform {
                    translation: emitter
                        .placement
                        .transform_point(wow_to_bevy(p.pos.to_array())),
                    rotation: emitter.placement.rotation * p.quat,
                    scale: Vec3::splat(size * inst_scale),
                }
            };
            for (e, mat) in &slot.meshes {
                if let Ok((mut t, mut g, mut vis, mut tag)) = draws.get_mut(*e) {
                    *t = tf;
                    *g = GlobalTransform::from(tf);
                    if *vis != Visibility::Inherited {
                        *vis = Visibility::Inherited;
                    }
                    *tag = MeshTag(crate::mesh_tag::alpha_bits(rgba[3]));
                }
                if let Some(m) = materials.get_mut(mat) {
                    m.extension.tint = Vec4::new(rgba[0], rgba[1], rgba[2], 1.0);
                }
            }
        }
    }
}
