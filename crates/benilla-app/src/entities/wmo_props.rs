//! The doodad props of a WMO-display GameObject: a transport's sail rig, rotor and cabin
//! furniture. A GameObject has no MODF placement and so no selected extra set: its props are
//! doodad set 0 (`Set_$DefaultGlobal`) alone, the only set either 1.12 transport authors (the ship
//! 134 props, the zeppelin 1).
//!
//! Each prop spawns through the shared placed-model assembler with its doodad-local transform,
//! parented under the GameObject, so it rides the moving boat, despawns with it and inherits its
//! off-map hide; animated props take the ordinary doodad animation host. A prop with a collision
//! hull gets a body-less collider child, which avian attaches to the transport's kinematic body,
//! and the mover's ride-attach walks the parent chain, so standing on a crate is standing on the
//! boat; hull-less furniture is walk-through, as in the reference.
//!
//! Its dressing is entity-owned, never the terrain path's world bake. Emitters ride their host bone
//! and re-anchor to the prop's live position every frame, as the reference rebuilds its
//! `translate(−emitterPos)` draw matrix, and take no `EmitterFade`, whose centre is a fixed world
//! point; vmangos streams transports map-wide, so the far-clip wall in the particle sim is what
//! bounds the deck lanterns. M2 point lights are children at the prop-local position.
//!
//! An indoor-group prop takes the interior SH-probe lane, folded once through the host's spawn
//! pose. That stays exact on a mover: the ambient word has no direction, the diffuse lobe rides a
//! fixed world axis, and a MOLR lobe's gain is a relative distance. No 1.12 transport WMO authors a
//! MOLT light, so no lobe direction freezes at spawn.

use benilla_assets::coords::{wmo_doodad_local, wow_to_bevy};
use benilla_assets::{DoodadBase, M2Model, WmoModel};
use benilla_protocol::EntityKind;
use bevy::prelude::*;

use crate::net::NetEntity;
use benilla_assets::m2_url;
use benilla_world::lighting::{PropProbeSlot, PropProbes};
use benilla_world::model_render::ShadeSel;
use benilla_world::particles;
use benilla_world::terrain_stream::{
    build_collider_task, doodad_ground_shade, fold_interior_probe, hex_word, m2_anim_bound,
    m2_fade, placement_collider_data, point_light, spawn_model_entities, PendingCollider,
    PropLobeLight, ShadeResolve, SpawnedModel, TerrainStreamer,
};

use super::{GameObjects, ModelHandle, VisualAttached};

/// Marks an entity the prop resolver has visited, units and M2 GameObjects included, so it is
/// never scanned again.
#[derive(Component)]
pub(super) struct WmoPropsResolved;

/// A WMO GameObject's props still waiting on their M2s; removed when the list drains.
#[derive(Component)]
pub(super) struct WmoProps(Vec<WmoProp>);

/// One pending prop, with its WMO-local placement.
struct WmoProp {
    handle: Handle<M2Model>,
    local: Transform,
    /// Set for a doodad of an indoor group (MODR ownership, [`DoodadBase::Interior`]); `None` is
    /// the deck lane, lit as an exterior doodad.
    interior: Option<InteriorLane>,
}

/// An interior prop's light inputs, kept WMO-local so the fold composes them through the host's
/// pose at spawn, frames after the resolve.
struct InteriorLane {
    /// `cap96(MODD.colour)`, the ambient word.
    ambient: [f32; 3],
    /// `floor112(MODD.colour)`, the diffuse word on the fixed interior axis.
    diffuse: [f32; 3],
    /// The owning group's MOLR omnis as (WMO-local position, colour × intensity, attenStart,
    /// attenEnd); empty on every 1.12 transport.
    lights: Vec<(Vec3, [f32; 3], f32, f32)>,
}

/// Resolve the set-0 props of each newly attached WMO-display GameObject. Runs after
/// `attach_entity_visuals`, whose `VisualAttached` means the `WmoModel` is resident.
#[allow(clippy::type_complexity)] // the visit-once attach-gate query
pub(super) fn resolve_wmo_gameobject_props(
    mut commands: Commands,
    gameobjects: Option<Res<GameObjects>>,
    wmos: Res<Assets<WmoModel>>,
    asset_server: Res<AssetServer>,
    pending: Query<(Entity, &NetEntity), (With<VisualAttached>, Without<WmoPropsResolved>)>,
) {
    for (entity, net) in &pending {
        commands.entity(entity).insert(WmoPropsResolved);
        if net.kind != EntityKind::GameObject {
            continue;
        }
        let Some(wmo) = net
            .display_id
            .and_then(|d| gameobjects.as_deref()?.models.get(&d))
            .and_then(|dm| match &dm.handle {
                ModelHandle::Wmo(h) => wmos.get(h),
                _ => None,
            })
        else {
            continue; // no display, an M2 display, or the asset gone
        };
        let Some(set) = wmo.doodad_sets.first() else {
            continue;
        };
        let props: Vec<WmoProp> = wmo
            .doodads
            .iter()
            .enumerate()
            .skip(set.start as usize)
            .take(set.count as usize)
            .filter(|(_, d)| !d.model.is_empty()) // the MODN name did not resolve
            .map(|(di, d)| WmoProp {
                handle: asset_server.load(m2_url(&d.model)),
                local: wmo_doodad_local(d.position, d.orientation, d.scale),
                // The indoor classification and MODD colour words; MOLR refs resolve here to
                // WMO-local lights, so the spawn needs the WMO asset no further.
                interior: match wmo.doodad_base.get(di) {
                    Some(DoodadBase::Interior(b)) => Some(InteriorLane {
                        ambient: b.ambient,
                        diffuse: b.diffuse,
                        lights: b
                            .light_refs
                            .iter()
                            .filter_map(|&li| wmo.lights.get(li as usize))
                            .filter(|l| l.is_omni())
                            .map(|l| {
                                (
                                    wow_to_bevy(l.position),
                                    l.color.map(|c| c * l.intensity.max(0.0)),
                                    l.attenuation_start,
                                    l.attenuation_end,
                                )
                            })
                            .collect(),
                    }),
                    _ => None,
                },
            })
            .collect();
        if !props.is_empty() {
            // Named by entity as well as display: each entity is visited once, so two lines for
            // one display are two ships of the same model.
            info!(
                "wmo props: {} set-0 doodads resolved for display {} on {entity}",
                props.len(),
                net.display_id.unwrap_or_default()
            );
            commands.entity(entity).insert(WmoProps(props));
        }
    }
}

/// Spawn each pending prop under its GameObject as its M2 lands, with no per-frame budget: a map
/// holds only a handful of transports.
pub(super) fn spawn_wmo_gameobject_props(
    mut commands: Commands,
    m2s: Res<Assets<M2Model>>,
    mut forms: ResMut<benilla_world::model_forms::ModelForms>,
    mut mats: benilla_world::model_render::M2BatchMaterials,
    mut uv_reg: ResMut<benilla_world::doodad_anim::UvAnimMaterials>,
    mut tint_reg: ResMut<benilla_world::doodad_anim::TintAnimMaterials>,
    mut anim_table: ResMut<benilla_world::mat_anim_table::MatAnimTable>,
    mut probes: ResMut<PropProbes>,
    // An exterior prop's one-shot MCSH sample, as the reference's queue drain `0x698c50` takes it.
    streamer: Option<Res<TerrainStreamer>>,
    adt_tiles: Res<Assets<benilla_assets::AdtTile>>,
    time: Res<Time>,
    mut hosts: Query<(Entity, &GlobalTransform, &mut WmoProps)>,
) {
    let Some((mat_cache, materials, light)) = mats.pieces() else {
        return; // no shared light buffer yet
    };
    let light = &light;
    // Animated props take their phase from their spawn time.
    let now = time.elapsed_secs();
    for (entity, host_gt, mut props) in &mut hosts {
        // The host's pose this frame, the base of the interior fold and the emitters' first
        // placement; the fold is rigid-motion invariant and the emitters follow their owner.
        let host_world = host_gt.compute_transform();
        let (mut hulls, mut emitters, mut ribbons, mut lights, mut interior) =
            (0u32, 0u32, 0u32, 0u32, 0u32);
        props.0.retain(|prop| {
            let Some(m) = m2s.get(&prop.handle) else {
                return true; // still loading; retry next frame
            };
            // Priority 0, the entity lane's, like any creature walking into view.
            let ready = if benilla_world::doodad_anim::wants_rig(m) {
                forms.require_rigged(&prop.handle, 0)
            } else {
                forms.require_static(&prop.handle, 0)
            };
            if !ready {
                return true; // forms still building
            }
            // A boneless prop's glow card, a world root, follows an anchor child at the prop's
            // placement, spawned only for a model with billboard batches.
            let card_owner = m.submeshes.iter().any(|s| s.billboard.is_some()).then(|| {
                let anchor = commands.spawn((prop.local, Visibility::default())).id();
                commands.entity(entity).add_child(anchor);
                anchor
            });
            // An exterior prop's sun scale is its own doodad's, not its host's: the reference
            // samples MCSH once at the doodad's footprint as its pending queue drains it
            // (`0x698c50`) and freezes 1.0 lit or 0.5 shadowed. A tile not resident answers lit, as
            // the reference's null-tile legs do; the interior lane never reads the selector.
            let shade = if prop.interior.is_some() {
                ShadeSel::Matte
            } else {
                let world = host_world.mul_transform(prop.local).translation;
                match streamer
                    .as_deref()
                    .map(|st| doodad_ground_shade(st, &adt_tiles, world))
                {
                    Some(ShadeResolve::Ready(true)) => ShadeSel::Shaded,
                    Some(ShadeResolve::Ready(false)) | None => ShadeSel::Matte,
                    // A resident tile still decoding defers the prop a frame, as on the terrain.
                    Some(ShadeResolve::Pending) => return true,
                }
            };
            let (radius, center) = m2_fade(&m.bounds, prop.local.scale.x);
            let anim_bound = m2_anim_bound(&m.bounds);
            // Fold an interior prop's light once, through the host's current pose (module docs).
            let interior_slot = prop.interior.as_ref().and_then(|lane| {
                let ref_point = host_world.mul_transform(prop.local).transform_point(center);
                let world_lights: Vec<PropLobeLight> = lane
                    .lights
                    .iter()
                    .map(|&(pos, color_i, atten_start, atten_end)| PropLobeLight {
                        pos: host_world.transform_point(pos),
                        color_i,
                        atten_start,
                        atten_end,
                    })
                    .collect();
                let slot = probes.alloc(fold_interior_probe(
                    lane.ambient,
                    lane.diffuse,
                    ref_point,
                    &world_lights,
                ));
                if slot.is_none() {
                    let (live, peak) = probes.occupancy();
                    warn_once!(
                        "interior-prop probe table full (live {live}, peak {peak}); \
                         cabin props fall back to exterior light"
                    );
                }
                slot
            });
            // The identity only names the kind and the parts: a pick on the ship answers with the
            // GameObject's own `WorldObject`.
            let object = std::sync::Arc::new(benilla_world::interact::WorldObject {
                kind: benilla_world::model_render::ModelKind::Doodad,
                label: prop
                    .handle
                    .path()
                    .map(|p| p.path().to_string_lossy().into_owned())
                    .unwrap_or_default(),
                id: 0,
                // The prop's lane and probe, as a terrain prop's identity names them.
                detail: match (&prop.interior, interior_slot) {
                    (Some(lane), Some(slot)) => format!(
                        "WMO gameobject prop · interior amb {} dif {} · {} MOLR · probe slot {slot}",
                        hex_word(lane.ambient),
                        hex_word(lane.diffuse),
                        lane.lights.len(),
                    ),
                    (Some(_), None) => {
                        "WMO gameobject prop · interior, NO PROBE SLOT (table full — sky-lit)".into()
                    }
                    (None, _) => "WMO gameobject prop · sky-lit".into(),
                },
            });
            let SpawnedModel {
                entities: ents,
                host,
                ..
            } = spawn_model_entities(
                &mut commands,
                mat_cache,
                materials,
                light,
                &m.submeshes,
                forms.slices(&prop.handle),
                prop.local, // doodad-local; the parent composes the world pose
                &object,
                shade,
                interior_slot,
                // No draw-set gate, whose `EmitterFade` measures from a fixed world point; the
                // assembler builds its sphere from `radius` and `center` instead.
                None,
                radius,
                center,
                anim_bound,
                Some((m, now)),
                &mut uv_reg,
                &mut tint_reg,
                &mut anim_table,
                true, // the entity-hosted lane: these props are lit by their own def
                card_owner,
                // Never diverted: every divert lane bakes world transforms.
                None,
                None,
            );
            commands.entity(entity).add_children(&ents);
            // The slot's component hook returns it to the table whoever despawns the prop.
            if let (Some(slot), Some(&first)) = (interior_slot, ents.first()) {
                commands.entity(first).insert(PropProbeSlot(slot));
                interior += 1;
            }
            // Solid cargo: the hull bakes the prop-local placement into its vertices, and the
            // child has no `RigidBody`, so avian attaches it to the nearest body ancestor, the
            // transport's kinematic body (a static WMO GameObject's `Static` one).
            if let Some((verts, tris)) = placement_collider_data(m.collision.as_ref(), &prop.local)
            {
                // No visibility components: a hull draws nothing, and Bevy's hierarchy check only
                // flags a visible child under a bare parent, never the reverse.
                let hull = commands
                    .spawn((
                        Transform::IDENTITY,
                        PendingCollider::new(build_collider_task(verts, tris), None, false),
                    ))
                    .id();
                commands.entity(entity).add_child(hull);
                hulls += 1;
            }
            // The entity-owned dressing (module docs), which needs a root entity to ride.
            if let Some(&root) = ents.first() {
                let placement = host_world.mul_transform(prop.local);
                for em in &m.emitters {
                    // The emitter rides its host bone's anchor when the prop animates, else the
                    // prop root; either owner's propagated transform carries the hull's motion.
                    let owner = host
                        .as_ref()
                        .and_then(|h| h.anchor(em.def.bone))
                        .map_or((root, [0.0; 3]), |a| (a, em.bone_pivot));
                    if let Some(e) = particles::spawn_emitter(
                        &mut commands,
                        em,
                        placement,
                        particles::EmitterFrames {
                            owner: Some(owner),
                            // The cloud anchors at the prop, as the reference re-anchors every
                            // cloud to the emitter's live position; a bone never drags it.
                            anchor: Some(root),
                            // A placed prop's model is destroyed when its placement unloads.
                            on_owner_loss: particles::OwnerLoss::Free,
                            // A WMO prop is lit by the `CMapDoodadDef` provider (`0x6a8050`), not
                            // the `WENTITY` one this edge feeds; its interior words, folded at
                            // spawn, do not reach its emitters.
                            light_node: None,
                            alpha: None,
                        },
                        // A placed prop re-rolls its variation every play window, so the emitter
                        // reads the slot and clip time off the host's live player each frame.
                        match host.as_ref().and_then(|h| h.arm) {
                            Some(arm) => particles::EmitClock::Host(arm),
                            None => particles::EmitClock::Pinned,
                        },
                    ) {
                        // Parented, so the off-map hide and the despawn reach the flame; the sim
                        // writes its transform after propagation, so the parent never moves it.
                        commands.entity(entity).add_child(e);
                        emitters += 1;
                    }
                }
                // Ribbon trails ride the host bone the same way and despawn with their owner.
                for rb in &m.ribbons {
                    let (owner, use_pivot) = host
                        .as_ref()
                        .and_then(|h| h.anchor(rb.def.bone))
                        .map_or((root, false), |a| (a, true));
                    if benilla_world::ribbons::spawn_ribbon(
                        &mut commands,
                        rb,
                        owner,
                        use_pivot,
                        placement.scale.max_element(),
                        benilla_world::ribbons::RibbonSeq::Host(entity),
                        // No model-alpha source: a placed prop is always drawn.
                        None,
                        // No fade sphere, as for the emitters; `simulate_ribbons` still bounds a
                        // fade-less trail at the far-clip wall. A trail reads its owner's render
                        // alpha, not the `Visibility` the off-map hide writes, but no 1.12
                        // transport prop authors a ribbon.
                        None,
                    )
                    .is_some()
                    {
                        ribbons += 1;
                    }
                }
                // M2 point lights, children at the prop-local position, carried with the hull. A
                // directional light feeds an ambient term, and a static 0 visibility key is dark.
                for l in m.lights.iter().map(|l| &l.def) {
                    if !l.casts() {
                        continue;
                    }
                    let glow = commands
                        .spawn((
                            point_light(l.diffuse_color, l.diffuse_intensity),
                            Transform::from_translation(
                                prop.local.transform_point(wow_to_bevy(l.position)),
                            ),
                            Visibility::default(),
                        ))
                        .id();
                    commands.entity(entity).add_child(glow);
                    lights += 1;
                }
            }
            false // spawned; drop from the pending list
        });
        if hulls + emitters + ribbons + lights + interior > 0 {
            // What this host's props authored; one host's props may log over several frames.
            info!(
                "wmo props: {hulls} cargo hulls, {emitters} emitters, {ribbons} ribbons, \
                 {lights} lights, {interior} interior-lane props (riding host {entity})"
            );
        }
        if props.0.is_empty() {
            commands.entity(entity).remove::<WmoProps>();
        }
    }
}
