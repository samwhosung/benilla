//! Spawns streamed placements as their model assets land, under a per-frame time and count budget.

mod assemble;
mod fx;
pub(crate) mod prop_light;

pub use assemble::{spawn_model_entities, SpawnedModel};
pub use fx::point_light;
use fx::{
    emitter_fade, spawn_emitters_for, spawn_lights_for, spawn_ribbons_for, spawn_wmo_lights_for,
};

use std::sync::Arc;
use std::time::Instant;

use benilla_assets::coords::{wmo_doodad_local, wow_to_bevy};
use benilla_assets::{AdtTile, DoodadBase, M2Model, WmoModel};
use benilla_formats::{world_to_tile, M2Bounds};
use bevy::camera::primitives::Aabb;
use bevy::prelude::*;

use crate::collision::{camera_layers, walk_layers, GroundDecalSurface, PickOccluder};
use crate::doodad_anim::wants_rig;
use crate::interact::WorldObject;
use crate::lighting::SharedLightBuffer;
use crate::lighting::{PropProbeSlot, PropProbes};
use crate::liquid::{spawn_wmo_liquids, LiquidAssets};
use crate::model_forms::{FormSlices, ModelForms, ModelKey, WANT_SKINNED, WANT_STATIC};
use crate::model_render::ModelKind;
use crate::model_render::ShadeSel;
use crate::wmo_portal::{WmoGroupVis, WmoPortalInstance, WmoRoom};
use benilla_assets::m2_url;
use benilla_assets::materials::WowModelMaterial;

use super::collider::{
    build_collider_task, doodad_bodies_disabled, doodad_hulls_bare, placement_collider_data,
    PendingCollider,
};
use super::merge::{MergeSite, StaticMerge};
use super::weld::{hull_weld_disabled, HullWelds};
use super::{
    doodad_ground_shade, ModelHandle, Placements, ShadeResolve, TerrainStreamer, SPAWN_BUDGET,
};
use prop_light::{fold_interior_probe, PropLight, PropLobeLight, WmoDoodadInst};

/// Placements (models or WMO props) spawned per frame once paced: ~150 × ~50 µs of downstream
/// cost each is about half a 60 Hz frame of render work.
const SPAWN_COUNT_CAP: usize = 150;

/// The nested resources of [`spawn_loaded_placements`], under Bevy's 16-parameter limit.
type SpawnTables<'w> = (
    ResMut<'w, PropProbes>,
    ResMut<'w, crate::terrain_stream::StreamActivity>,
    Res<'w, crate::terrain_stream::ViewFocus>,
    ResMut<'w, ModelForms>,
    ResMut<'w, HullWelds>,
    ResMut<'w, StaticMerge>,
    // `None` only under `WOW_STATIC_GX=0`, when the retained pass is not armed.
    Option<ResMut<'w, crate::static_gx::StaticGx>>,
);

pub(super) fn spawn_loaded_placements(
    mut commands: Commands,
    placements: ResMut<Placements>,
    m2s: Res<Assets<M2Model>>,
    wmos: Res<Assets<WmoModel>>,
    materials: ResMut<Assets<WowModelMaterial>>,
    asset_server: Res<AssetServer>,
    shared_light: Option<Res<SharedLightBuffer>>,
    mut meshes: ResMut<Assets<Mesh>>,
    // The shared per-kind liquid materials WMO group liquids spawn on; absent without game data.
    liquid_assets: Option<Res<LiquidAssets>>,
    // For the MCSH ground-shade lookup; `stream_terrain`, chained before this, owns it mutably.
    streamer: Res<TerrainStreamer>,
    adt_tiles: Res<Assets<AdtTile>>,
    time: Res<Time>,
    mut uv_reg: ResMut<crate::doodad_anim::UvAnimMaterials>,
    mut tint_reg: ResMut<crate::doodad_anim::TintAnimMaterials>,
    mut anim_table: ResMut<crate::mat_anim_table::MatAnimTable>,
    tables: SpawnTables,
) {
    let (mut probes, mut activity, focus, mut forms, mut welds, mut merge, mut staticgx) = tables;
    let Some(shared_light) = shared_light else {
        return;
    };
    // Steady state: the pending count, kept by register, handoff and release, skips the walk.
    if placements.pending_spawns == 0 {
        return;
    }
    let t0 = Instant::now();
    // The count cap bounds the render-side wave a time budget misses; off until the focus is
    // paced, as the loading cover absorbs the burst and a cap would only lengthen the reveal.
    let count_cap = if focus.paced {
        SPAWN_COUNT_CAP
    } else {
        usize::MAX
    };
    let mut spawned_n = 0usize;
    // The anim clock origin: each instance's phase is its spawn time, as in the reference.
    let now = time.elapsed_secs();
    let light = &shared_light.0;
    let materials = materials.into_inner();
    let Placements {
        by_id,
        materials: mat_cache,
        pending_spawns,
    } = placements.into_inner();

    // At most `SPAWN_BUDGET` per frame, resumed next frame through the `spawned` flags; the check
    // follows the work, so every frame spawns at least one.
    let deadline = Instant::now() + SPAWN_BUDGET;

    'placements: for (&unique_id, p) in by_id.iter_mut() {
        // 1. The model's own geometry, once; a WMO also resolves its doodad props for step 2.
        if !p.spawned {
            let entities = match &p.model {
                ModelHandle::M2(h) => {
                    let Some(m) = m2s.get(h) else {
                        continue; // model still loading (or missing): try next frame
                    };
                    let key = ModelKey::from(h);
                    let kinds = WANT_STATIC | if wants_rig(m) { WANT_SKINNED } else { 0 };
                    if !forms.require(
                        key,
                        kinds,
                        placement_priority(&streamer, p.transform.translation),
                    ) {
                        continue;
                    }
                    // The MCSH ground shade, by a global world→tile→chunk lookup at the doodad's
                    // origin as the reference does, whatever tile registered it. An ADT doodad is
                    // the fixed 1.0 family, one class with the exterior WMO prop: its
                    // `CMapDoodadDef` `[+0xa4]` is only ever 0.0, 0.5 or 1.0. The 2.5 boost and its
                    // 3.3333/s ramp (`0x69e770`) belong to the entity light node (vtable
                    // `0x810810`), which a doodad lacks; the 2.5's one reader is `0x69e4ad`.
                    let shade =
                        match doodad_ground_shade(&streamer, &adt_tiles, p.transform.translation) {
                            ShadeResolve::Ready(true) => ShadeSel::Shaded,
                            ShadeResolve::Ready(false) => ShadeSel::Matte,
                            // The ground tile is still decoding: wait, rather than bake the lit
                            // fallback into a straddling tree whose tile lands a frame later.
                            ShadeResolve::Pending => continue,
                        };
                    let (radius, center) = m2_fade(&m.bounds, p.transform.scale.x);
                    let anim_bound = m2_anim_bound(&m.bounds);
                    // The placement's one draw-set gate; a map doodad has no instance or rooms.
                    let fade = emitter_fade(p.transform, (radius, center), None, None);
                    // The pick identity, carried by whichever lane takes a batch.
                    let object = Arc::new(WorldObject {
                        kind: ModelKind::Doodad,
                        label: handle_label(h),
                        id: unique_id,
                        detail: format!("emitters: {}", m.emitters.len()),
                    });
                    let SpawnedModel {
                        entities: mut ents,
                        by_batch,
                        host,
                    } = spawn_model_entities(
                        &mut commands,
                        mat_cache,
                        materials,
                        light,
                        &m.submeshes,
                        FormSlices {
                            stat: forms.static_meshes(key).unwrap_or(&[]),
                            skin: forms.skinned_meshes(key),
                        },
                        p.transform,
                        &object,
                        shade,
                        None, // map doodad: exterior sky lighting (no interior probe)
                        Some(&fade),
                        radius,
                        center,
                        anim_bound,
                        Some((m, now)),
                        &mut uv_reg,
                        &mut tint_reg,
                        &mut anim_table,
                        false, // world-static: not the entity-hosted lane
                        None,  // world-static placement: cards bake their world pivot
                        Some((&mut *merge, MergeSite::Doodad { owner: p.owner })),
                        staticgx
                            .as_deref_mut()
                            .map(|gx| (gx, crate::static_gx::GxSite::Doodad { owner: p.owner })),
                    );
                    // ADT doodads are exterior scene: from inside a WMO they draw only through a
                    // portal window (`0x683700`, fed only by the per-window walk `0x682fa0`).
                    // Deviation: tagged per submesh, where the reference tests the whole object,
                    // because a placement has no root entity to carry the bound. The anim-host root
                    // is skipped: with no geometry it has no `Aabb`, and would only pad the counts.
                    let anim_root = host.as_ref().map(|h| h.root);
                    for e in ents.iter().filter(|e| Some(**e) != anim_root) {
                        commands
                            .entity(*e)
                            .insert(crate::exterior_cull::ExteriorScene);
                    }
                    // The collision hull, welded into the owner tile's batch, which carries the
                    // `PickOccluder` clamp. A hull-less model (a tree canopy) stays pick-through,
                    // as the reference's world trace tests only collision-flagged doodads.
                    if let Some((verts, tris)) = (!doodad_bodies_disabled())
                        .then(|| placement_collider_data(m.collision.as_ref(), &p.transform))
                        .flatten()
                    {
                        if hull_weld_disabled() {
                            ents.push(
                                commands
                                    .spawn((
                                        PendingCollider::new(
                                            build_collider_task(verts, tris),
                                            None,
                                            !doodad_hulls_bare(),
                                        ),
                                        PickOccluder,
                                    ))
                                    .id(),
                            );
                        } else {
                            welds.add_tile(p.owner, verts, tris);
                        }
                    }
                    spawn_emitters_for(
                        &mut commands,
                        &m.emitters,
                        p.transform,
                        host.as_ref(),
                        &fade,
                        &mut ents,
                    );
                    spawn_ribbons_for(
                        &mut commands,
                        &m.ribbons,
                        p.transform,
                        host.as_ref(),
                        ents.first().copied(),
                        &fade,
                    );
                    spawn_lights_for(&mut commands, &m.lights, p.transform, None, &mut ents);
                    tag_world_object(&mut commands, &ents, &object);
                    if let Some((target, r)) = fade_near_target() {
                        let pos = p.transform.translation;
                        let d = bevy::math::Vec2::new(pos.x, pos.z).distance(target);
                        if d <= r {
                            log_fade_near(
                                "doodad",
                                &handle_label(h),
                                unique_id,
                                pos,
                                d,
                                (radius, p.transform.scale.x),
                                &by_batch,
                            );
                        }
                    }
                    ents
                }
                ModelHandle::Wmo(h) => {
                    let Some(m) = wmos.get(h) else {
                        continue;
                    };
                    // Static forms only: WMO group geometry never skins.
                    let key = ModelKey::from(h);
                    let prio = placement_priority(&streamer, p.transform.translation);
                    if !forms.require(key, WANT_STATIC, prio) {
                        continue;
                    }
                    let has_portals = !m.portal_refs.is_empty() && !m.portal_infos.is_empty();
                    // No authored bounds, so a WMO never size-fades; the far clip bounds it.
                    let fade = emitter_fade(p.transform, (f32::INFINITY, Vec3::ZERO), None, None);
                    let object = Arc::new(WorldObject {
                        kind: ModelKind::Wmo,
                        label: handle_label(h),
                        id: unique_id,
                        detail: String::new(),
                    });
                    // The portal-cull instance, spawned before the batches: the retained pass keys
                    // its region on it, and the region lives as long as it does.
                    let instance = (has_portals || m.wmo_id != 0).then(|| {
                        commands
                            .spawn(WmoPortalInstance {
                                handle: h.clone(),
                                world_from_local: p.transform.compute_affine(),
                                name_set: p.name_set,
                                visible: vec![true; m.group_nav.len()],
                                // Fogged only once a flood puts the group on the chain.
                                interior_fog: vec![false; m.group_nav.len()],
                                liquid_visited: vec![false; m.group_nav.len()],
                                // MOGP `groupLiquid`: a room wholly submerged, with no liquid grid.
                                flooded: m
                                    .group_nav
                                    .iter()
                                    .map(|g| {
                                        (g.group_liquid != benilla_formats::NO_GROUP_LIQUID)
                                            .then(|| {
                                                benilla_formats::LiquidKind::from_nibble(
                                                    (g.group_liquid & 0xf) as u8,
                                                )
                                            })
                                            .flatten()
                                    })
                                    .collect(),
                            })
                            .id()
                    });
                    let SpawnedModel {
                        entities: mut ents,
                        by_batch,
                        ..
                    } = spawn_model_entities(
                        &mut commands,
                        mat_cache,
                        materials,
                        light,
                        &m.submeshes,
                        FormSlices {
                            stat: forms.static_meshes(key).unwrap_or(&[]),
                            skin: None,
                        },
                        p.transform,
                        &object,
                        ShadeSel::Matte, // unread: WMO lights on the FFP N·L path
                        None, // WMO groups carry their own per-submesh interior flag + batch class
                        Some(&fade),
                        f32::INFINITY,
                        Vec3::ZERO,
                        None, // no authored M2 box: group geometry never animates
                        None, // not an M2: its doodad props animate below
                        &mut uv_reg,
                        &mut tint_reg,
                        &mut anim_table,
                        false, // world-static: not the entity-hosted lane
                        None,  // world-static placement: cards bake their world pivot
                        Some((
                            &mut *merge,
                            MergeSite::Wmo {
                                uid: unique_id,
                                groups: &m.submesh_group,
                                portal_gated: has_portals,
                            },
                        )),
                        // A retained region keyed by the instance; without one there is no PVS key.
                        instance.and_then(|i| {
                            staticgx.as_deref_mut().map(|gx| {
                                (
                                    gx,
                                    crate::static_gx::GxSite::Wmo {
                                        instance: i,
                                        groups: &m.submesh_group,
                                    },
                                )
                            })
                        }),
                    );
                    if let Some((target, r)) = fade_near_target() {
                        let pos = p.transform.translation;
                        let d = bevy::math::Vec2::new(pos.x, pos.z).distance(target);
                        if d <= r {
                            log_fade_near(
                                "wmo",
                                &handle_label(h),
                                unique_id,
                                pos,
                                d,
                                (f32::INFINITY, p.transform.scale.x),
                                &by_batch,
                            );
                        }
                    }
                    // A world WMO is exterior scene too (`0x6856c0`, fed by the same walk
                    // `0x682fa0`): from inside one building, another draws only through a portal
                    // window. The tag is unconditional; the camera's own building is exempted per
                    // frame from `CameraInteriorClaim`.
                    for e in &ents {
                        commands
                            .entity(*e)
                            .insert(crate::exterior_cull::ExteriorScene);
                    }
                    // Tie each group's submeshes (`by_batch` is index-parallel with them) to the
                    // instance. A portal-less building keeps every group visible but still gets an
                    // instance when it has a WMOAreaTable identity, for the interior down-ray.
                    if let Some(instance) = instance {
                        if has_portals {
                            // One shared key per group, not per submesh (a city has ~100k).
                            let group_key: Vec<Arc<[u16]>> = (0..m.group_nav.len() as u16)
                                .map(|g| Arc::from([g].as_slice()))
                                .collect();
                            for (&entity, &group) in by_batch.iter().zip(&m.submesh_group) {
                                let Some(entity) = entity else { continue };
                                let Some(groups) = group_key.get(group as usize).cloned() else {
                                    continue;
                                };
                                commands
                                    .entity(entity)
                                    .insert(WmoGroupVis { instance, groups });
                            }
                        }
                        // Held for the props, which spawn later, and for any instance: it also
                        // scopes the building's embedded pools, portals or not.
                        p.portal_instance = Some(instance);
                        ents.push(instance); // despawns with the placement
                    }
                    // The embedded MLIQ liquid, spawned a group at a time so each surface takes its
                    // room's cull key: a culled room's lava goes with it.
                    for (gi, lq) in m.group_liquids.iter().enumerate() {
                        let Some(lq) = lq else { continue };
                        let first = ents.len();
                        spawn_wmo_liquids(
                            &mut commands,
                            std::iter::once(lq),
                            liquid_assets.as_deref(),
                            &mut meshes,
                            p.transform,
                            // Interior (`MOGI & 0x48 == 0`): the pool takes the room's fog.
                            m.group_bounds.get(gi).is_some_and(|g| g.interior),
                            crate::liquid::WmoPool::new(
                                p.portal_instance.map(|instance| WmoRoom {
                                    instance,
                                    group: gi as u16,
                                }),
                                &p.transform,
                                m.group_bounds.get(gi),
                            ),
                            // The root's MOMT diffColor table: an interior pool's body colour.
                            &m.material_diff_color,
                            &mut ents,
                        );
                        // Another building's pool is exterior scene like its walls (`0x6856c0`),
                        // tagged unconditionally: an instance-less placement is never the camera's
                        // room, so the tag alone is right for it.
                        for &e in &ents[first..] {
                            commands
                                .entity(e)
                                .insert(crate::exterior_cull::ExteriorScene);
                        }
                        if let Some(instance) = p.portal_instance {
                            let groups: Arc<[u16]> = Arc::from([gi as u16].as_slice());
                            for &e in &ents[first..] {
                                // With an instance the pool rides its room and the exemption;
                                // `WmoGroupVis` moves it to the model-visibility authority.
                                commands.entity(e).insert(WmoGroupVis {
                                    instance,
                                    groups: groups.clone(),
                                });
                            }
                        }
                    }
                    // Two colliders, each on its layer: the reference gathers different faces for
                    // the body (drops DETAIL) and the camera (drops NOCAMCOLLIDE, keeps DETAIL).
                    if let Some((verts, tris)) =
                        placement_collider_data(m.collision.as_ref(), &p.transform)
                    {
                        ents.push(
                            commands
                                .spawn((
                                    PendingCollider::new(
                                        build_collider_task(verts, tris),
                                        Some(walk_layers()),
                                        true,
                                    ),
                                    // The walk faces take the selection ring (floors, steps).
                                    GroundDecalSurface,
                                    // Clamps the mouse pick. The reference's occluder faces are
                                    // MOPY reject-mask 0x84; the walk bake (0x04) is the nearest.
                                    PickOccluder,
                                ))
                                .id(),
                        );
                    }
                    if let Some((verts, tris)) =
                        placement_collider_data(m.collision_camera.as_ref(), &p.transform)
                    {
                        ents.push(
                            commands
                                .spawn(PendingCollider::new(
                                    build_collider_task(verts, tris),
                                    Some(camera_layers()),
                                    true,
                                ))
                                .id(),
                        );
                    }
                    // MOLT lights: they light nearby models and the walls over their baked MOCV.
                    spawn_wmo_lights_for(
                        &mut commands,
                        &m.lights,
                        &m.group_light_refs,
                        p.portal_instance,
                        p.transform,
                        &mut ents,
                    );
                    tag_world_object(&mut commands, &ents, &object);
                    p.doodads = resolve_wmo_doodads(m, p.doodad_set, p.transform, &asset_server);
                    ents
                }
            };
            p.spawned = true;
            p.entities = entities;
            // The model landed; a WMO's just-resolved props join the pending count.
            *pending_spawns += p.doodads.iter().filter(|d| !d.spawned).count();
            *pending_spawns -= 1;
            activity.placements_spawned += 1;
            spawned_n += 1;
            // Budget spent: the rest, this placement's props included, waits for the next frame.
            if Instant::now() >= deadline || spawned_n >= count_cap {
                break 'placements;
            }
        }

        // 2. Each WMO doodad prop as its M2 lands; its entities join `p.entities`.
        let portal_instance = p.portal_instance;
        for d in &mut p.doodads {
            if d.spawned {
                continue;
            }
            let Some(m) = m2s.get(&d.handle) else {
                continue; // this prop's M2 still loading
            };
            let key = ModelKey::from(&d.handle);
            let kinds = WANT_STATIC | if wants_rig(m) { WANT_SKINNED } else { 0 };
            if !forms.require(
                key,
                kinds,
                placement_priority(&streamer, d.transform.translation),
            ) {
                continue;
            }
            // An exterior prop samples the MCSH at its footprint, as the reference's per-frame
            // refresh `0x698c50` does: matte on lit ground, shaded under MCSH shadow, never the
            // 2.5 boost (its one reader, `0x69e4ad`, is unreachable from the WMO render band).
            let shade = if matches!(d.light, PropLight::Interior { .. }) {
                ShadeSel::Matte // interior lane; the selector is unread
            } else {
                match doodad_ground_shade(&streamer, &adt_tiles, d.transform.translation) {
                    ShadeResolve::Ready(true) => ShadeSel::Shaded,
                    ShadeResolve::Ready(false) => ShadeSel::Matte,
                    // The ground tile is still decoding: defer this prop a frame.
                    ShadeResolve::Pending => continue,
                }
            };
            let (radius, center) = m2_fade(&m.bounds, d.transform.scale.x);
            let anim_bound = m2_anim_bound(&m.bounds);
            // The prop's one gate, with its building's window exemption and rooms: a particles-only
            // prop's host has no other way to know it is furniture, not exterior scene.
            let fade = emitter_fade(
                d.transform,
                (radius, center),
                portal_instance,
                Some(&d.groups),
            );
            // An interior prop's light, folded once into its SH probe: the reference refolds at
            // each light commit, but every input here is static. The reference point is the placed
            // M2 box centre, the `[def+0x5c]` anchor of `0x6952a0`/`0x713680`.
            let interior_slot = match &d.light {
                PropLight::Exterior => None,
                PropLight::Interior {
                    ambient,
                    diffuse,
                    lights,
                } => {
                    let ref_point = d.transform.transform_point(center);
                    let coeffs = fold_interior_probe(*ambient, *diffuse, ref_point, lights);
                    let slot = probes.alloc(coeffs);
                    if slot.is_none() {
                        // Once per burst, not per prop: a city WMO can flood thousands in a frame.
                        let (live, peak) = probes.occupancy();
                        warn_once!(
                            "interior-prop probe table full (live {live}, peak {peak});                              overflowing props fall back to exterior light"
                        );
                    }
                    slot
                }
            };
            let object = Arc::new(WorldObject {
                kind: ModelKind::Doodad,
                label: handle_label(&d.handle),
                id: unique_id,
                detail: format!(
                    "emitters: {} · WMO prop · {}",
                    m.emitters.len(),
                    d.light.inspector_label()
                ),
            });
            let SpawnedModel {
                entities: mut ents,
                host,
                ..
            } = spawn_model_entities(
                &mut commands,
                mat_cache,
                materials,
                light,
                &m.submeshes,
                FormSlices {
                    stat: forms.static_meshes(key).unwrap_or(&[]),
                    skin: forms.skinned_meshes(key),
                },
                d.transform,
                &object,
                shade,
                interior_slot, // interior props light off their folded probe, not the sky
                Some(&fade),
                radius,
                center,
                anim_bound,
                Some((m, now)),
                &mut uv_reg,
                &mut tint_reg,
                &mut anim_table,
                false, // world-static: not the entity-hosted lane
                None,  // world-static placement: cards bake their world pivot
                Some((
                    &mut *merge,
                    MergeSite::Prop {
                        uid: unique_id,
                        groups: &d.groups,
                        slot: interior_slot,
                    },
                )),
                // The retained-pass region is the building's instance, its PVS key and lifetime;
                // without one the prop takes the merge or entity path, tallied.
                match (portal_instance, staticgx.as_deref_mut()) {
                    (Some(instance), Some(gx)) => Some((
                        gx,
                        crate::static_gx::GxSite::Prop {
                            instance,
                            groups: &d.groups,
                            slot: interior_slot,
                        },
                    )),
                    (None, Some(gx)) => {
                        gx.tally_prop_declined(true);
                        None
                    }
                    _ => None,
                },
            );
            // A despawn hook returns the slot to the table. A prop whose every batch merged spawns
            // a bare carrier for it, since a blob cannot own one prop's slot.
            if let Some(slot) = interior_slot {
                let owner = match ents.first() {
                    Some(&first) => first,
                    None => {
                        let carrier = commands.spawn_empty().id();
                        ents.push(carrier);
                        carrier
                    }
                };
                commands.entity(owner).insert(PropProbeSlot(slot));
            }
            // Cull the prop with the rooms that name it: the reference admits a WMO's doodads to
            // the frame per group its portal walk reaches (`0x685d70` → `0x6838f0` over the
            // group's MODR list), so a prop is drawn from any of them. The anim-host root is
            // skipped, as at the ADT site.
            let anim_root = host.as_ref().map(|h| h.root);
            if let (Some(instance), false) = (portal_instance, d.groups.is_empty()) {
                for &entity in ents.iter().filter(|e| Some(**e) != anim_root) {
                    commands.entity(entity).insert((
                        WmoGroupVis {
                            instance,
                            groups: d.groups.clone(),
                        },
                        // Exterior scene with its building, exempt in the camera's own; a prop no
                        // group names has no instance key, so it stays untagged.
                        crate::exterior_cull::ExteriorScene,
                    ));
                }
            }
            // Solid to body and camera iff it has a collision hull, like a map doodad; welded per
            // placement, as every prop despawns with its building.
            if let Some((verts, tris)) = (!doodad_bodies_disabled())
                .then(|| placement_collider_data(m.collision.as_ref(), &d.transform))
                .flatten()
            {
                if hull_weld_disabled() {
                    ents.push(
                        commands
                            .spawn((
                                PendingCollider::new(
                                    build_collider_task(verts, tris),
                                    None,
                                    !doodad_hulls_bare(),
                                ),
                                PickOccluder,
                            ))
                            .id(),
                    );
                } else {
                    welds.add_prop(unique_id, verts, tris);
                }
            }
            spawn_emitters_for(
                &mut commands,
                &m.emitters,
                d.transform,
                host.as_ref(),
                &fade,
                &mut ents,
            );
            spawn_ribbons_for(
                &mut commands,
                &m.ribbons,
                d.transform,
                host.as_ref(),
                ents.first().copied(),
                &fade,
            );
            spawn_lights_for(
                &mut commands,
                &m.lights,
                d.transform,
                fade.room.as_ref(), // the prop's glow rides its rooms like its mesh
                &mut ents,
            );
            tag_world_object(&mut commands, &ents, &object);
            p.entities.extend(ents);
            d.spawned = true;
            *pending_spawns -= 1;
            activity.placements_spawned += 1;
            spawned_n += 1;
            if Instant::now() >= deadline || spawned_n >= count_cap {
                break 'placements;
            }
        }
    }
    activity.spawn_ms += t0.elapsed().as_secs_f32() * 1000.0;
}

/// A WMO placement's doodad props: set 0, always shown, plus its MODF-selected set.
fn resolve_wmo_doodads(
    wmo: &WmoModel,
    doodad_set: u16,
    wmo_world: Transform,
    asset_server: &AssetServer,
) -> Vec<WmoDoodadInst> {
    let mut ranges: Vec<(u32, u32)> = wmo
        .doodad_sets
        .first()
        .map(|s| (s.start, s.count))
        .into_iter()
        .collect();
    if doodad_set != 0 {
        if let Some(s) = wmo.doodad_sets.get(doodad_set as usize) {
            ranges.push((s.start, s.count));
        }
    }
    let mut out = Vec::new();
    for (start, count) in ranges {
        for (di, d) in wmo
            .doodads
            .iter()
            .enumerate()
            .skip(start as usize)
            .take(count as usize)
        {
            if d.model.is_empty() {
                continue; // MODN name offset didn't resolve
            }
            let local = wmo_doodad_local(d.position, d.orientation, d.scale);
            // The asset-level base, with its MOLR lights moved into world space for the fold.
            let light = match wmo.doodad_base.get(di) {
                Some(DoodadBase::Interior(b)) => PropLight::Interior {
                    ambient: b.ambient,
                    diffuse: b.diffuse,
                    lights: b
                        .light_refs
                        .iter()
                        .filter_map(|&li| wmo.lights.get(li as usize))
                        .filter(|l| l.is_omni())
                        .map(|l| PropLobeLight {
                            pos: wmo_world.transform_point(wow_to_bevy(l.position)),
                            color_i: l.color.map(|c| c * l.intensity.max(0.0)),
                            atten_start: l.attenuation_start,
                            atten_end: l.attenuation_end,
                        })
                        .collect(),
                },
                _ => PropLight::Exterior,
            };
            out.push(WmoDoodadInst {
                handle: asset_server.load(m2_url(&d.model)),
                transform: wmo_world.mul_transform(local),
                // Every group whose MODR names the prop, inverted at asset load.
                groups: wmo.doodad_groups.get(di).cloned().unwrap_or_default(),
                light,
                spawned: false,
            });
        }
    }
    out
}

/// `WOW_FADE_NEAR="x,y,r"`: a server-coordinate point (as `.go xyz` takes) and a radius in yards;
/// every placement spawning within it logs its model, fade radius, scale and diverted batches.
fn fade_near_target() -> Option<(bevy::math::Vec2, f32)> {
    static T: std::sync::OnceLock<Option<(bevy::math::Vec2, f32)>> = std::sync::OnceLock::new();
    *T.get_or_init(|| {
        let s = std::env::var("WOW_FADE_NEAR").ok()?;
        let mut it = s.split(',').map(|v| v.trim().parse::<f32>().ok());
        let (x, y, r) = (it.next()??, it.next()??, it.next()??);
        let b = wow_to_bevy([x, y, 0.0]);
        Some((bevy::math::Vec2::new(b.x, b.z), r))
    })
}

/// One `[fade-near]` line; `diverted` counts the batches that left the entity path.
fn log_fade_near(
    kind: &str,
    label: &str,
    unique_id: u32,
    pos: Vec3,
    d: f32,
    (radius, scale): (f32, f32),
    by_batch: &[Option<Entity>],
) {
    let diverted = by_batch.iter().filter(|e| e.is_none()).count();
    eprintln!(
        "[fade-near] {kind} {label} uid={unique_id} d={d:.1} radius={radius:.2} scale={scale:.2} \
         batches={} diverted={diverted} pos=({:.1},{:.1},{:.1})",
        by_batch.len(),
        pos.x,
        pos.y,
        pos.z,
    );
}

/// A placement's render-forms priority, lower sooner: its Chebyshev tile distance to the focus,
/// above the entity lane's band so a unit coming into view never queues behind scenery.
/// `translation` is Bevy space; `world_to_tile` takes WoW ground coordinates.
fn placement_priority(streamer: &TerrainStreamer, translation: Vec3) -> i32 {
    let (tx, ty) = world_to_tile(-translation.z, -translation.x);
    let d = (tx as i32 - streamer.focus.0)
        .abs()
        .max((ty as i32 - streamer.focus.1).abs());
    16 + d * 16
}

fn handle_label<A: Asset>(handle: &Handle<A>) -> String {
    handle
        .path()
        .map(|p| p.path().to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Tags a placement's spawned entities, fx included, with its [`WorldObject`] for the inspector; a
/// diverted batch carries the same Arc in its lane instead.
fn tag_world_object(commands: &mut Commands, ents: &[Entity], object: &Arc<WorldObject>) {
    for &e in ents {
        commands.entity(e).insert((**object).clone());
    }
}

/// An animated M2 placement's cull bound: the authored header box, its extent over every animation
/// (Bevy model-local), as the joints carry the vertices off the bind-pose bound.
///
/// Deviation: the box, where the reference tests its circumsphere (`0x683700` → `0x682ef0` →
/// `0x686b80` on the `[rec+0x5c]` centre and `[rec+0x68]` radius × scale), because Bevy culls by
/// box and the box is tighter.
pub fn m2_anim_bound(bounds: &Option<M2Bounds>) -> Option<Aabb> {
    let b = bounds.as_ref()?;
    // The basis swap permutes and negates axes, so min/max have to be re-derived, not mapped.
    let (a, c) = (wow_to_bevy(b.bbox_min), wow_to_bevy(b.bbox_max));
    let (lo, hi) = (a.min(c), a.max(c));
    // A degenerate (all-zero) authored box counts as none.
    (hi.cmpgt(lo).any()).then(|| Aabb::from_min_max(lo, hi))
}

/// An M2's fade sphere: authored sphere radius × scale, centred on the authored box (model-local).
pub fn m2_fade(bounds: &Option<M2Bounds>, scale: f32) -> (f32, Vec3) {
    match bounds {
        Some(b) => {
            let c = [
                (b.bbox_min[0] + b.bbox_max[0]) * 0.5,
                (b.bbox_min[1] + b.bbox_max[1]) * 0.5,
                (b.bbox_min[2] + b.bbox_max[2]) * 0.5,
            ];
            (b.sphere_radius * scale, wow_to_bevy(c))
        }
        None => (f32::INFINITY, Vec3::ZERO),
    }
}
