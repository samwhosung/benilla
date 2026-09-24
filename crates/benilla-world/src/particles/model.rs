//! Model particles: an emitter whose record names a geometry model draws each particle as a small
//! instance of it (Whirlwind's blades), placed after the sim each frame (`0x7b4840` →
//! `0x7b4510`): its quaternion at its position, scaled by over-life size, colour in a per-instance
//! tint, alpha in the `MeshTag`. Each blended instance sorts by its own depth in the transparent
//! pass; the reference visits an emitter's particles back to front only under `rt+0x1ac & 0x10`,
//! and the order its model pass draws them in is untraced. Rigged models are not animated: every
//! spell geometry model probed is static.

use benilla_assets::coords::wow_to_bevy;
use benilla_assets::M2Model;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::mesh::MeshTag;
use bevy::prelude::*;

use crate::model_render::{model_material, MaterialCache, ShadeSel};
use benilla_assets::materials::WowModelMaterial;

use super::{ChildDraw, ParticleEmitter};

/// Cap on one emitter's instances (the corpus stays under ~25/s × 3 s); the rest don't draw.
const MAX_INSTANCES: usize = 128;

/// One pooled instance: the model's submeshes as flat world-root [`ChildDraw`] entities, so the
/// direct transform write is exact, each with its own tint material.
pub(super) struct ModelInstance {
    pub(super) meshes: Vec<(Entity, Handle<WowModelMaterial>)>,
}

/// Grow and place each model-particle emitter's instances; runs after
/// [`super::sim::simulate_particles`], so they land on this frame's positions.
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
        // Gated: the sim hid these instances, and `Visibility::Inherited` below would re-show
        // them. The sim clears `gated` first, so a thawed pool resumes the frame it wakes.
        if emitter.gated {
            continue;
        }
        let Some(model) = models.get(&geometry) else {
            continue; // still loading: particles simulate meanwhile, nothing draws yet
        };
        // Built now, not paced: a spell's visual must not lag its cast, and these models are tiny.
        forms.ensure_now_static(&geometry, &model.submeshes, &mut mesh_assets);
        let rung = emitter.owner_rung();
        let want = emitter.particles.len().min(MAX_INSTANCES);
        while emitter.model_instances.len() < want {
            let stat_forms = forms.slices(&geometry).stat;
            let meshes = model
                .submeshes
                .iter()
                .enumerate()
                .map(|(pi, sub)| {
                    // A fresh material per instance: the over-life ramp writes its tint.
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
                        // Lit like every entity M2 (`0x69e280`).
                        ShadeSel::Lit,
                        0,
                        None,
                        None, // the over-life tint owns the colour, never the M2Color loop
                        None,
                        None,
                        false,
                        false, // an effect model is never a skybox
                        &light.0,
                        None, // no animated loop, so no placement key
                    );
                    // Realized now: the over-life ramp writes it every frame.
                    crate::model_render::lazy::realize(&mut materials, material.id());
                    // The owner-last rung (`ParticleEmitter::owner_rung`), bucketed up: bevy keys
                    // pipelines on `depth_bias`, only buckets are pre-warmed, and a bucket never
                    // sorts a shard under its quad cloud. Opaque batches take no sort bias.
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
            // No texture atlas here: only colour and size reach a model particle.
            let ol = emitter.def.over_life.sample(u);
            let (mut rgba, size) = (ol.color, ol.size);
            // The owning model's render alpha, into the instance's `MeshTag` alpha.
            rgba[3] *= emitter.render_alpha();
            let tf = if anchored {
                Transform {
                    // World mode: through the ride frame (identity off a transport).
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
