//! Builds a placed model's submesh entities: materials, fade, mesh tags and the doodad anim host.

use std::sync::Arc;

use benilla_assets::{M2Model, ModelSubmesh};
use benilla_formats::ModelBlend;
use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::NoAutoAabb;
use bevy::mesh::MeshTag;
use bevy::prelude::*;
use bevy::render::render_resource::Buffer;

use crate::billboard::BillboardCard;
use crate::doodad_anim::DoodadAnimHost;
use crate::mesh_tag::alpha_bits;
use crate::model_fade::DoodadFade;
use crate::model_render::{model_material, MaterialCache, ShadeSel};
use crate::model_render::{ModelKind, ModelPart};
use benilla_assets::materials::WowModelMaterial;

/// What one placement's anim host armed, for the fx spawned beside its submeshes.
pub struct PlacementHost {
    /// `(bone, anchor)` for each bone something rides, every emitter and ribbon bone minted before
    /// the pose buffer attaches.
    anchors: Vec<(u16, Entity)>,
    /// The anim root, despawned with the placement but not geometry: a caller tagging what the
    /// placement draws leaves it out.
    pub(crate) root: Entity,
    /// The anim root if a sequence was armed, whose live player names the playing slot; `None` on
    /// the gseq-only tier, which reads slot 0.
    pub arm: Option<Entity>,
}

impl PlacementHost {
    /// The anchor for `bone`; `None` when nothing this placement spawns rides it.
    pub fn anchor(&self, bone: u16) -> Option<Entity> {
        self.anchors
            .iter()
            .find(|&&(b, _)| b == bone)
            .map(|&(_, e)| e)
    }
}

/// What [`spawn_model_entities`] spawned.
pub struct SpawnedModel {
    /// The despawn list: anim host, parts and world-root cards (a following card despawns itself).
    pub entities: Vec<Entity>,
    /// Index-parallel with `submeshes`: each batch's entity, `None` where it diverted or had no
    /// render form. The WMO path zips group indices against it, so a skipped batch holds its slot.
    pub by_batch: Vec<Option<Entity>>,
    /// The anim host, when the model animates.
    pub host: Option<PlacementHost>,
}

/// Spawns a model's submeshes at `transform`. A transport's props pass a doodad-local `transform`
/// and parent the result under the moving gameobject, as every reader uses `GlobalTransform`.
pub fn spawn_model_entities(
    commands: &mut Commands,
    mat_cache: &mut MaterialCache,
    materials: &mut Assets<WowModelMaterial>,
    light: &Buffer,
    submeshes: &[ModelSubmesh],
    // Index-parallel with `submeshes`; complete, as callers gate on `ModelForms::require`.
    forms: crate::model_forms::FormSlices<'_>,
    transform: Transform,
    // The placement's pick identity: a diverted batch spawns no entity, so its lane carries this.
    object: &Arc<crate::interact::WorldObject>,
    // The MCSH terrain shade at the base; WMO group geometry and interior props ignore it.
    shade: ShadeSel,
    // An interior M2 prop's SH-probe slot, carried in `MeshTag` on every batch, cards included.
    interior_slot: Option<u16>,
    // The placement's draw-set gate, shared with its emitters, ribbons, lights and anim host: a
    // meshless prop's only verdict. `None` on the entity-hosted lane, as a mover has no baked
    // world point to measure from; the mesh lane then builds a bare sphere from `radius`.
    fade: Option<&crate::particles::EmitterFade>,
    radius: f32,
    local_center: Vec3,
    // The all-animation box an animated placement is culled with; a static one ignores it.
    anim_bound: Option<Aabb>,
    // An M2 placement's model and anim clock origin; `None` for WMO group geometry.
    m2: Option<(&M2Model, f32)>,
    uv_reg: &mut crate::doodad_anim::UvAnimMaterials,
    tint_reg: &mut crate::doodad_anim::TintAnimMaterials,
    anim_table: &mut crate::mat_anim_table::MatAnimTable,
    // A WMO prop hosted on a streamed entity (a transport's cargo): every batch, cards included,
    // then gets `DoodadDefLit`, as its light is its own `CMapDoodadDef`'s, not its host's. This
    // is the lane; `card_owner` only says whether the prop has cards to follow.
    entity_hosted: bool,
    // On a streamed entity, the anchor a boneless model's cards follow instead of a baked pivot;
    // those cards stay world roots, out of the returned list.
    card_owner: Option<Entity>,
    // The merge buffer and this placement's site on the world-static lanes; `None` on a mover.
    mut merge: Option<(
        &mut super::super::merge::StaticMerge,
        super::super::merge::MergeSite<'_>,
    )>,
    // The retained-pass collector and site on the world-static lanes; `None` on a mover and on a
    // prop without an instance entity.
    mut staticgx: Option<(
        &mut crate::static_gx::StaticGx,
        crate::static_gx::GxSite<'_>,
    )>,
) -> SpawnedModel {
    // The caller's gate, else the bare sphere; every radius or centre read below goes through it.
    let fade = match fade {
        Some(f) => f.clone(),
        None => {
            crate::particles::EmitterFade::sphere(radius, transform.transform_point(local_center))
        }
    };
    let kind = object.kind;
    let is_wmo = kind == ModelKind::Wmo;
    let mut out = Vec::with_capacity(submeshes.len());
    // The anim host serves any joint consumer: a skinned part, an emitter or a ribbon. A model of
    // billboard cards alone gets none, as the billboard system owns their transforms.
    let mut host = m2
        .filter(|(m, _)| {
            submeshes.iter().any(|s| s.billboard.is_none())
                || !m.emitters.is_empty()
                || !m.ribbons.is_empty()
        })
        .and_then(|(m, _)| crate::doodad_anim::spawn_anim_host(commands, m, transform));
    // Captures freeze material-alpha clocks at 0, for deterministic frames.
    let mat_frozen = crate::dev_state::deterministic_run();
    let animated = host.is_some();
    let rig_root = host.as_ref().map(|h| h.root);
    if let Some(h) = &host {
        out.push(h.root);
    }
    let mut skinned_meshes: Vec<Entity> = Vec::new();
    // The parts with a `SkinnedTwin`, which the lazy rig's wake promotes to skinned.
    let mut lazy_parts: Vec<Entity> = Vec::new();
    // Every `continue` below leaves its batch's `None` in place.
    let mut by_batch: Vec<Option<Entity>> = vec![None; submeshes.len()];
    // Coplanar siblings ride one lane: two batches naming one skin section draw the same
    // triangles, which the reference draws from one vertex array under LEQUAL, so the later wins
    // exactly (`0x70c190`). Split across lanes, the two reach each vertex by different float
    // arithmetic and the depth test flickers, so neither diverts.
    let shared_geometry = shared_geometry(
        &submeshes
            .iter()
            .map(|s| s.geometry.section)
            .collect::<Vec<_>>(),
    );
    for (batch_idx, sub) in submeshes.iter().enumerate() {
        // A missing form is a broken caller contract: skip the batch rather than panic.
        let Some((stat_mesh, stat_aabb)) = forms.stat.get(batch_idx) else {
            continue;
        };
        // The authored order biases the transparent sort, so coplanar layers draw in file order;
        // a WMO batch also takes it into the clip-z nudge, as the reference draws MOBA batches in
        // strict file order (`0x6b4f10`/`0x6b5190`).
        let batch_order = u16::try_from(batch_idx + 1).unwrap_or(u16::MAX);
        let interior = sub.interior || interior_slot.is_some();
        // A billboard card culls by its `0x04` flag like any batch: the bone turns +X to the
        // viewer and the cull is GL_BACK/CCW (`0x70c2b3`), so a −X card is meant to go unseen.
        let two_sided = sub.two_sided;
        // An interior M2 prop's batches carry its SH-probe slot, cards included: the reference
        // shades every batch of an object through one light node.
        let interior_probe = !is_wmo && interior_slot.is_some();
        // Only a non-card interior batch is steady; a card still fades with its lamp.
        let steady_interior_prop = interior_probe && sub.billboard.is_none();
        // A batch whose UV or tint loop differs by sequence gets a material keyed by this
        // placement's host, since placements re-roll their variations independently. Without a
        // host it shares and stays at the built seed.
        let per_seq = sub.uv_seq.is_some() || sub.rgb_seq.is_some();
        let seq_owner = per_seq.then_some(rig_root).flatten();
        let cutout = model_material(
            mat_cache,
            materials,
            sub.texture.clone(),
            sub.blend,
            two_sided,
            is_wmo,
            interior,
            sub.emissive,
            sub.additive,
            false,
            sub.no_depth_write,
            sub.no_depth_test,
            sub.fog_policy,
            sub.env_map, // texture_unit_lookup > 2 ⇒ the runtime generates this batch's UVs
            shade,
            batch_order,
            sub.uv_anim.as_ref(),
            sub.rgb_anim.as_ref(),
            sub.wmo_batch,
            sub.sidn,
            sub.window,
            false, // the world streamer never spawns a skybox
            light,
            seq_owner,
        );
        // The fade's blend twin, the cutout itself when already blended or never fading. A multiply
        // batch (Mod, Mod2x) keeps its cutout too: it reads no alpha, so the shader fades its
        // colour toward the blend identity, the reference's preset-5 mechanism.
        let blend = if steady_interior_prop
            || matches!(
                sub.blend,
                ModelBlend::Blend | ModelBlend::Mod | ModelBlend::Mod2x
            ) {
            cutout.clone()
        } else {
            // The source blend decides the twin's cutout: only AlphaKey alpha-tests while fading.
            model_material(
                mat_cache,
                materials,
                sub.texture.clone(),
                sub.blend,
                two_sided,
                is_wmo,
                interior,
                sub.emissive,
                sub.additive,
                true,
                sub.no_depth_write,
                sub.no_depth_test,
                sub.fog_policy,
                sub.env_map, // texture_unit_lookup > 2 ⇒ the runtime generates this batch's UVs
                shade,
                batch_order,
                sub.uv_anim.as_ref(),
                sub.rgb_anim.as_ref(),
                sub.wmo_batch,
                sub.sidn,
                sub.window,
                false, // the world streamer never spawns a skybox
                light,
                seq_owner,
            )
        };
        // One classification for the census tally and the diverts, so the two cannot drift.
        let class = crate::static_merge::BatchClass {
            excluded: animated
                || sub.billboard.is_some()
                || sub.alpha_anim.is_some()
                || seq_owner.is_some()
                || sub.uv_anim.is_some()
                || sub.rgb_anim.is_some(),
            interior_prop: interior_probe,
            order_free: matches!(sub.blend, ModelBlend::Opaque | ModelBlend::AlphaTest)
                && !sub.additive,
            never_fade: fade.radius > crate::model_fade::NEVER_FADE_RADIUS,
        };
        // A merged fader stays in the reference's fading state, the blend twin with per-vertex
        // alpha, which at fade 1.0 matches the cutout pixel for pixel. A steady interior prop
        // keeps the cutout and an infinite sphere, as it carries no `DoodadFade`.
        let merge_mat = if class.never_fade || steady_interior_prop {
            &cutout
        } else {
            &blend
        };
        let merge_sphere = if steady_interior_prop {
            Vec4::new(0.0, 0.0, 0.0, f32::INFINITY)
        } else {
            Vec4::from((fade.center, fade.radius))
        };
        if crate::static_merge::census_enabled() {
            let key = merge
                .as_ref()
                .and_then(|(_, site)| site.census_key(batch_idx, merge_mat, &transform));
            crate::static_merge::tally(&class, is_wmo, sub.geometry.positions.len(), key);
        }
        // The retained-pass divert, tried before the merge: an eligible order-free batch leaves
        // bevy_pbr for `static_gx`, a doodad fader with its fade seed so it can return as an entity
        // inside its fade band. A refused batch falls through with its slot held; `why` is the
        // census label for one left on the entity path.
        let mut why: &'static str = "gx-off";
        if let Some((gx, site)) = staticgx.as_mut() {
            let facts = if !crate::static_gx::enabled() {
                None
            } else if shared_geometry[batch_idx] {
                why = "shared-geometry";
                None
            } else {
                why = match site {
                    crate::static_gx::GxSite::Doodad { .. } if is_wmo => "doodad-site-wmo",
                    crate::static_gx::GxSite::Doodad { .. } if class.excluded => "no-merge-anim",
                    crate::static_gx::GxSite::Doodad { .. } if !class.merges() => {
                        "no-merge-transparent"
                    }
                    crate::static_gx::GxSite::Doodad { .. } if class.interior_prop => {
                        "interior-prop"
                    }
                    crate::static_gx::GxSite::Doodad { .. } => "fader-lane-off",
                    crate::static_gx::GxSite::Wmo { .. } if !is_wmo => "wmo-site-m2",
                    crate::static_gx::GxSite::Wmo { .. } if class.excluded => "wmo-no-merge-anim",
                    crate::static_gx::GxSite::Wmo { .. } if !class.merges() => {
                        "wmo-no-merge-transparent"
                    }
                    crate::static_gx::GxSite::Wmo { .. } => "wmo-no-group",
                    crate::static_gx::GxSite::Prop { .. } if is_wmo => "prop-site-wmo",
                    crate::static_gx::GxSite::Prop { .. } if class.excluded => "prop-no-merge-anim",
                    crate::static_gx::GxSite::Prop { .. } if !class.merges() => {
                        "prop-no-merge-transparent"
                    }
                    crate::static_gx::GxSite::Prop { .. } => "exterior-fader-prop",
                };
                match site {
                    crate::static_gx::GxSite::Doodad { owner }
                        if !is_wmo && class.merges() && !class.interior_prop =>
                    {
                        let fade_seed = (!class.never_fade
                            && !crate::static_gx::fade_lane_disabled())
                        .then(|| crate::static_gx::GxFadeSeed {
                            radius: fade.radius,
                            local_center,
                            stat_mesh: stat_mesh.clone(),
                            aabb: *stat_aabb,
                            cutout: cutout.clone(),
                            blend: blend.clone(),
                        });
                        // A never-fader enters bare, a fader only with its seed.
                        (class.never_fade || fade_seed.is_some())
                            .then_some((*owner, None, None, fade_seed))
                    }
                    crate::static_gx::GxSite::Wmo { instance, groups }
                        if is_wmo && class.merges() =>
                    {
                        groups.get(batch_idx).map(|&g| {
                            (
                                (0, 0), // WMO items release by instance death, never by tile
                                Some(crate::static_gx::GxWmoBatch {
                                    instance: *instance,
                                    group: g,
                                    interior,
                                    class: sub.wmo_batch,
                                    sidn: sub.sidn,
                                    window: sub.window,
                                    batch_order,
                                }),
                                None,
                                None,
                            )
                        })
                    }
                    crate::static_gx::GxSite::Prop {
                        instance,
                        groups,
                        slot,
                    } if !is_wmo && class.merges() => {
                        if steady_interior_prop || class.never_fade {
                            Some((
                                (0, 0), // prop items release by instance death too
                                None,
                                Some(crate::static_gx::GxPropBatch {
                                    instance: *instance,
                                    groups: std::sync::Arc::clone(groups),
                                    slot: *slot,
                                }),
                                None,
                            ))
                        } else {
                            gx.tally_prop_declined(false); // an exterior fader prop
                            None
                        }
                    }
                    _ => None,
                }
            };
            if let Some((owner, wmo, prop, fade)) = facts {
                why = "gx-declined";
                if gx.divert(crate::static_gx::GxBatch {
                    geometry: &sub.geometry,
                    transform,
                    object,
                    aabb: *stat_aabb,
                    owner,
                    texture: sub.texture.clone(),
                    blend: sub.blend,
                    two_sided,
                    unlit: sub.emissive,
                    fog_policy: sub.fog_policy,
                    env_map: sub.env_map,
                    no_depth_write: sub.no_depth_write,
                    no_depth_test: sub.no_depth_test,
                    shade,
                    wmo,
                    prop,
                    fade,
                }) {
                    continue;
                }
            }
        }
        // The merge divert: an order-free static batch joins its site's blob, its slot left
        // `None` and a fader's fade riding the baked sphere. WMO group geometry is refused.
        if let Some((merge, site)) = merge.as_mut() {
            if crate::terrain_stream::merge::merge_enabled()
                && !shared_geometry[batch_idx]
                && class.merges()
                // Faders merge only under `WOW_MERGE_FADERS=1`: a fader blob's depth write hides
                // the per-entity faders behind its translucent pixels.
                && (class.never_fade
                    || steady_interior_prop
                    || crate::terrain_stream::merge::merge_faders_enabled())
                && merge.divert(
                    site,
                    batch_idx,
                    merge_mat,
                    &sub.geometry,
                    transform,
                    merge_sphere,
                    sub.blend,
                    kind,
                    object,
                )
            {
                continue;
            }
        }
        // Register both materials for the UV scroll (idempotent per material). A per-sequence set
        // replaces the shared loop when there is a host to read; a period-0 loop is a constant the
        // seed already wrote, so it stays out.
        if let (Some(seqs), Some(host)) = (sub.uv_seq.as_ref(), seq_owner) {
            for id in [cutout.id(), blend.id()] {
                crate::doodad_anim::register_uv(
                    uv_reg,
                    anim_table,
                    materials,
                    id,
                    crate::doodad_anim::UvLoop::PerSeq {
                        seqs: seqs.clone(),
                        host,
                    },
                );
            }
        } else if let Some(anim) = sub.uv_anim.as_ref().filter(|a| a.period > 0.0) {
            for id in [cutout.id(), blend.id()] {
                crate::doodad_anim::register_uv(
                    uv_reg,
                    anim_table,
                    materials,
                    id,
                    crate::doodad_anim::UvLoop::Shared(Some(anim.clone())),
                );
            }
        }
        // Likewise for the animated M2Color RGB tint.
        if let (Some(seqs), Some(host)) = (sub.rgb_seq.as_ref(), seq_owner) {
            for id in [cutout.id(), blend.id()] {
                crate::doodad_anim::register_tint(
                    tint_reg,
                    anim_table,
                    materials,
                    id,
                    crate::doodad_anim::TintLoop::PerSeq {
                        seqs: seqs.clone(),
                        host,
                    },
                );
            }
        } else if let Some(anim) = sub.rgb_anim.as_ref().filter(|a| a.period > 0.0) {
            for id in [cutout.id(), blend.id()] {
                crate::doodad_anim::register_tint(
                    tint_reg,
                    anim_table,
                    materials,
                    id,
                    crate::doodad_anim::TintLoop::Shared(anim.clone()),
                );
            }
        }
        // `MeshTag`: an interior prop's probe slot through `mesh_tag::probe_bits`, else the fade
        // alpha at 1.0. One predicate feeds both the tag and `InteriorProbePayload` below, so the
        // payload's writer and readers cannot disagree.
        let probe_slot = interior_slot.filter(|_| interior_probe);
        let mesh_tag = match probe_slot {
            Some(slot) => MeshTag(crate::mesh_tag::probe_bits(slot)),
            None => MeshTag(alpha_bits(1.0)),
        };
        // A billboard card's transform is the billboard system's; it still fades with its doodad,
        // from its pivot, where the mesh is centred (`ZERO`).
        let (entity, fade_center) = if let Some(info) = &sub.billboard {
            // An animated doodad's card follows its billboard bone's anchor; a static one keeps
            // the baked pivot.
            let card = match host.as_mut().and_then(|h| h.anchor(commands, info.bone)) {
                Some(anchor) => BillboardCard::following_joint(info, anchor),
                None => match card_owner {
                    Some(anchor) => BillboardCard::following(info, anchor),
                    None => BillboardCard::new(info, transform),
                },
            };
            let mut card_entity = commands.spawn((
                Mesh3d(stat_mesh.clone()),
                MeshMaterial3d(cutout.clone()),
                Transform::from_translation(transform.transform_point(info.pivot)),
                ModelPart {
                    kind,
                    blend: sub.blend,
                },
                crate::model_render::EntityPathWhy(why),
                // The picker's triangles, as the render forms are `RENDER_WORLD`-only.
                crate::interact::PickMesh(sub.geometry.clone()),
                mesh_tag,
                card,
            ));
            // The build-time Aabb, inserted here: `calculate_bounds` can race extraction of a
            // `RENDER_WORLD` form, and the exterior cull fails open without a bound.
            if let Some(aabb) = stat_aabb {
                card_entity.insert((*aabb, NoAutoAabb));
            }
            (card_entity.id(), Vec3::ZERO)
        } else {
            // A part spawns static with its skinned twin beside it; the draw gate swaps the twin in
            // at the placement's first wake. A part without a twin never promotes.
            let skinned_mesh = animated
                .then(|| forms.skin.and_then(|s| s.get(batch_idx)).cloned())
                .flatten();
            let mut part_entity = commands.spawn((
                Mesh3d(stat_mesh.clone()),
                MeshMaterial3d(cutout.clone()),
                transform,
                ModelPart {
                    kind,
                    blend: sub.blend,
                },
                crate::model_render::EntityPathWhy(why),
                crate::interact::PickMesh(sub.geometry.clone()),
                mesh_tag,
            ));
            // An animated placement's joints move the vertices away from the bind-pose bound, so
            // it widens to the authored all-animation box; a card, which follows its joint, needs
            // none. `NoAutoAabb` keeps the bound: `calculate_bounds` recomputes it on
            // `Changed<Mesh3d>`, and the lazy rig swaps in a twin of bind-pose geometry.
            if let Some(aabb) =
                cull_bound(stat_aabb.as_ref(), animated.then_some(anim_bound).flatten())
            {
                part_entity.insert((aabb, NoAutoAabb));
            }
            if let Some(sm) = skinned_mesh {
                part_entity.insert(crate::doodad_anim::SkinnedTwin {
                    skinned: sm,
                    stat: stat_mesh.clone(),
                });
                lazy_parts.push(part_entity.id());
            }
            let entity = part_entity.id();
            if let Some(root) = rig_root {
                commands
                    .entity(entity)
                    .insert(crate::rig_palette::RigPart(root));
                // The draw gate's list is about visibility, so a not-yet-skinned part counts.
                skinned_meshes.push(entity);
            }
            (entity, local_center)
        };
        by_batch[batch_idx] = Some(entity);
        // Says the payload is a probe slot, so the exterior-payload writer passes it by; cards too,
        // as `entity_shade` reaches a card by walking up from its owner.
        if probe_slot.is_some() {
            commands
                .entity(entity)
                .insert(crate::mesh_tag::InteriorProbePayload);
        }
        // On the entity-hosted lane the batch is lit by its own doodad def, not its host; set
        // here as only this site reaches every batch, cards included.
        if entity_hosted {
            commands
                .entity(entity)
                .insert(crate::entity_shade::DoodadDefLit);
        }
        // The scan marker, by the registration's own predicate, so `tick_anim_materials` visits
        // only marked parts. A batch whose colour alpha animates or dims gets its own sampler,
        // which the visibility authority composes into the tag and the A ≤ 0 cull.
        if sub.uv_anim.as_ref().is_some_and(|a| a.period > 0.0)
            || sub.rgb_anim.as_ref().is_some_and(|a| a.period > 0.0)
            || seq_owner.is_some()
        {
            commands
                .entity(entity)
                .insert(crate::doodad_anim::AnimMatPart);
        }
        if let Some(anim) = &sub.alpha_anim {
            commands
                .entity(entity)
                .insert(crate::doodad_anim::MatAnim::new(
                    anim.clone(),
                    m2.map(|(_, now)| now).unwrap_or_default(),
                    mat_frozen,
                ));
        }
        // `DoodadFade` lets the fade system drive the tag; a steady interior prop never fades.
        if !steady_interior_prop {
            commands.entity(entity).insert(DoodadFade {
                radius: fade.radius,
                local_center: fade_center,
                cutout,
                blend,
            });
        }
        // A following card despawns with its owner and must stay a world root: keep it out.
        if sub.billboard.is_none() || card_owner.is_none() {
            out.push(entity);
        }
    }
    // Animation runs while a submesh is drawn or, for a meshless host, while its gate admits it.
    let arm = host.as_ref().and_then(|h| h.seq.map(|_| h.root));
    let placement_host = host.map(|mut h| {
        let now = m2.map(|(_, now)| now).unwrap_or_default();
        // Mint the emitter and ribbon bones' anchors while the pose buffer is in hand.
        if let Some((m, _)) = m2 {
            for bone in m
                .emitters
                .iter()
                .map(|e| e.def.bone)
                .chain(m.ribbons.iter().map(|r| r.def.bone))
            {
                h.anchor(commands, bone);
            }
        }
        commands.entity(h.root).insert(DoodadAnimHost {
            meshes: skinned_meshes,
            // The caller's gate itself, not a second construction of it.
            fade: fade.clone(),
            clip: h.clip,
            armed_at: now,
            // Born expired, so frame one runs the holder's `variationIdx = -1` arm over the
            // loader's var-0 seed, the reference's two-stage load.
            window_hi: f32::NEG_INFINITY,
            anim_id: h.anim_id,
            // Born parked: the spawn frame's default `Visibility` is not yet a verdict, so the
            // first real one decides the slot.
            active: false,
            parked_at: now,
        });
        // The lazy rig, only with skinnable parts: an emitter-only host never takes a slot.
        if !lazy_parts.is_empty() {
            commands.entity(h.root).insert(crate::doodad_anim::LazyRig {
                bones: h.bones(),
                ibp: h.inverse_bindposes.clone(),
                parts: lazy_parts,
            });
        }
        let root = h.root;
        let anchors = h.finish(commands);
        PlacementHost { anchors, root, arm }
    });
    SpawnedModel {
        entities: out,
        by_batch,
        host: placement_host,
    }
}

/// A placed submesh's cull bound: the bind-pose bound, unioned with the authored all-animation box
/// when animated. A union, as 152 of 9315 M2s author a box short of their own bind pose, which
/// the reference's circumsphere test swallows.
fn cull_bound(stat: Option<&Aabb>, anim: Option<Aabb>) -> Option<Aabb> {
    match (stat, anim) {
        (Some(s), Some(a)) => Some(Aabb::from_min_max(
            Vec3::from(s.min().min(a.min())),
            Vec3::from(s.max().max(a.max())),
        )),
        (Some(s), None) => Some(*s),
        (None, a) => a,
    }
}

/// Per batch, whether another batch of the model names the same skin section; a WMO batch (`None`)
/// never shares. Quadratic, as a model has tens of batches at most.
fn shared_geometry(sections: &[Option<u16>]) -> Vec<bool> {
    sections
        .iter()
        .map(|s| s.is_some() && sections.iter().filter(|o| *o == s).count() > 1)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `World\critter\birds\Bird01.m2` (`benilla-extract animboundscan`): a 1.2 × 1.8 × 0.23 yd
    /// bind-pose box and a 67 × 18 × 7 yd authored all-animation box.
    #[test]
    fn an_animated_placements_bound_covers_the_whole_flight_path() {
        // Bevy-space conversions of Bird01's two boxes (WoW (x,y,z) -> Bevy (-y, z, -x)).
        let bind = Aabb::from_min_max(
            Vec3::new(-0.863, 9.359, -3.536),
            Vec3::new(0.915, 9.586, -2.366),
        );
        let authored = Aabb::from_min_max(
            Vec3::new(-4.219, 8.816, -30.547),
            Vec3::new(13.571, 16.019, 36.605),
        );
        let widened = cull_bound(Some(&bind), Some(authored)).expect("a bound");
        // `Aabb` stores centre and half-extents, so the rebuilt corners need a float tolerance.
        const EPS: f32 = 1e-4;
        assert!((widened.min() - EPS).cmple(authored.min()).all());
        assert!((widened.max() + EPS).cmpge(authored.max()).all());
        // The bind-pose bound alone is ~37 yd short along the flight axis.
        assert!(authored.max().z - bind.max().z > 36.0);
    }

    #[test]
    fn a_short_authored_box_never_shrinks_the_bound() {
        let bind = Aabb::from_min_max(Vec3::splat(-5.0), Vec3::splat(5.0));
        let authored = Aabb::from_min_max(Vec3::new(-40.0, -1.0, -1.0), Vec3::new(40.0, 1.0, 1.0));
        let b = cull_bound(Some(&bind), Some(authored)).expect("a bound");
        assert!(Vec3::from(b.min()).abs_diff_eq(Vec3::new(-40.0, -5.0, -5.0), 1e-4));
        assert!(Vec3::from(b.max()).abs_diff_eq(Vec3::new(40.0, 5.0, 5.0), 1e-4));
    }

    #[test]
    fn a_static_placement_keeps_its_per_batch_bound() {
        let bind = Aabb::from_min_max(Vec3::splat(-1.0), Vec3::splat(1.0));
        let b = cull_bound(Some(&bind), None).expect("a bound");
        assert!(Vec3::from(b.min()).abs_diff_eq(Vec3::splat(-1.0), 1e-4));
        assert!(Vec3::from(b.max()).abs_diff_eq(Vec3::splat(1.0), 1e-4));
    }

    /// The ballista's shape: section 10 drawn twice (base and shine), section 4 once.
    #[test]
    fn both_batches_of_a_shared_section_are_flagged() {
        let flags = shared_geometry(&[Some(4), Some(10), Some(10), Some(7)]);
        assert_eq!(flags, vec![false, true, true, false]);
    }

    /// WMO batches carry no section, and `None == None` must not count as sharing one.
    #[test]
    fn section_less_batches_never_share() {
        assert_eq!(shared_geometry(&[None, None, None]), vec![false; 3]);
        assert_eq!(
            shared_geometry(&[None, Some(0), Some(0), None]),
            vec![false, true, true, false]
        );
    }
}
