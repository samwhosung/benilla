//! WMO-display GameObject doodad props — the ship's sails.
//!
//! A WMO's placed doodads (MODD — for the two 1.12 transports: the entire sail rig
//! `TRANSPORTSHIP_SAILS.MDX`, the zeppelin's rotor `ZEPANIMATION.MDX`, and the cabin furniture)
//! are M2 props the terrain path spawns for world-placed buildings but the gameobject path never
//! did, so ships sailed bare (director, 2026-07-17). A gameobject carries no MODF placement, so
//! there is no selected extra set: its props are doodad set 0 (`Set_$DefaultGlobal`, the
//! always-shown set) alone — and both transport WMOs author exactly one set (verified from the
//! extracted roots: ship 134 props / zeppelin 1, all in set 0).
//!
//! Each prop spawns through the shared placed-model assembler
//! ([`benilla_world::terrain_stream::spawn_model_entities`]) with its doodad-LOCAL transform, and the
//! result is parented under the streamed gameobject entity: transform propagation carries the
//! props with the moving boat, the entity's despawn cascades through them, and the transport's
//! off-map hide (`Visibility` on the root) is inherited. The animated props (sails, rotors) get
//! the ordinary doodad anim host — its joint/billboard/fade consumers all read propagated
//! `GlobalTransform`s, so riding a moving parent needs nothing special. A boneless prop's glow
//! card FOLLOWS an anchor child instead of baking a world pivot (`BillboardCard::following`).
//!
//! A prop with an authored collision hull is SOLID: its collider spawns as a body-less
//! child, which avian attaches to the nearest `RigidBody` ancestor — the transport's kinematic
//! body — so the cargo's collision sails with the deck (the same collide-iff-hull law as world
//! placements; hull-less furniture stays walk-through, exactly like the reference). The mover's
//! ride-attach walks the support's parent chain, so standing on a crate is standing on the boat.
//!
//! The prop's live dressing (closing the 0458 gaps) takes the ENTITY-owned shape, not the
//! terrain path's world-baked one:
//! - **Emitters** (the deck lanterns' flames) spawn with `owner` = the emitter's host-bone joint
//!   (or the prop's root submesh for a boneless model) and `anchor` = the prop root — the sim
//!   refreshes the placement from the owner's propagated `GlobalTransform` every frame and the
//!   risen cloud re-anchors to the prop's live position (the reference's per-frame
//!   `translate(−emitterPos)` draw-matrix rebuild: a moving model carries its flame). No
//!   [`benilla_world::particles::EmitterFade`] — that gate's centre is a baked world point, and a mover has
//!   none. The emitter entity parents under the gameobject, so the off-map hide reaches it and
//!   despawn cascades.
//!   ⚠ **This used to say the population is "bounded by server visibility instead". That was
//!   wrong, and a transport is the exact counter-example** (bug B39): vmangos
//!   streams transports **map-wide**, so a ship's deck lanterns are resident from anywhere on the
//!   map. Measured from Durotar: 56 emitters ticking and drawing at 4853–7080 yd, this boat's
//!   lanterns among them. The bound is now the far-clip wall, applied in
//!   [`benilla_world::particles`]' sim to every world-lane emitter regardless of `EmitterFade`.
//! - **M2 point lights** (the lantern's glow source) spawn as CHILDREN at the prop-local
//!   position — propagation carries the source with the hull, and the hide/despawn follow.
//! - **Interior cabin props** (an INDOOR-group MODD — ~118 of the ship's 133) take the interior
//!   SH-probe lane, folded ONCE through the host's spawn pose. Fold-once stays exact on a mover:
//!   the ambient word is directionless, the diffuse lobe rides the FIXED world axis (byte law —
//!   it never turns with the model), and a MOLR lobe's gain is a pure relative distance. Every
//!   1.12 transport WMO authors ZERO MOLT lights (verified from the four extracted roots), so
//!   there is no frozen-lobe-direction residual either; a future MOLR-authored mover would
//!   freeze its lobe directions at spawn — a noted residual, not worth live-refold machinery.

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

/// Marks a streamed entity the prop resolver has visited — once, ever (units and M2 gameobjects
/// included, so the resolve query never re-scans them). A WMO gameobject with props also gets
/// [`WmoProps`].
#[derive(Component)]
pub(super) struct WmoPropsResolved;

/// The not-yet-spawned doodad props of one WMO-display gameobject. Each spawns as its M2 asset
/// lands; the component is removed when the list drains (the per-frame query empties out).
#[derive(Component)]
pub(super) struct WmoProps(Vec<WmoProp>);

/// One pending prop: its M2 (loading async), its WMO-local placement, and — for an INDOOR-group
/// doodad — the interior-lane base to fold at spawn.
struct WmoProp {
    handle: Handle<M2Model>,
    local: Transform,
    /// `Some` iff the doodad belongs to an INDOOR group ([`DoodadBase::Interior`], MODR
    /// ownership resolved at asset load); `None` = the deck lane (plain matte, like an exterior
    /// terrain prop on lit ground — a boat is never MCSH-shadowed).
    interior: Option<InteriorLane>,
}

/// An interior prop's committed-light inputs, kept WMO-LOCAL so the spawn-time fold can compose
/// them through the host's live pose (the resolve and the spawn are frames apart on a mover).
struct InteriorLane {
    /// `cap96(MODD.colour)` — the ambient word.
    ambient: [f32; 3],
    /// `floor112(MODD.colour)` — the diffuse word, committed on the fixed interior axis.
    diffuse: [f32; 3],
    /// The owning group's MOLR omnis: (WMO-local Bevy position, colour × intensity, attenStart,
    /// attenEnd). Empty on every 1.12 transport (zero MOLT authored — see the module doc).
    lights: Vec<(Vec3, [f32; 3], f32, f32)>,
}

/// Resolve the MODD prop list of every freshly-attached WMO-display gameobject (set 0 — see the
/// module doc for why there is never a selected extra set). Runs after `attach_entity_visuals`:
/// `VisualAttached` on a modeled gameobject means its parts were built, which means the `WmoModel`
/// asset is resident (the display holds a strong handle).
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
            continue; // no display / M2 display / (shouldn't happen) asset gone
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
            .filter(|(_, d)| !d.model.is_empty()) // MODN name offset didn't resolve
            .map(|(di, d)| WmoProp {
                handle: asset_server.load(m2_url(&d.model)),
                local: wmo_doodad_local(d.position, d.orientation, d.scale),
                // Interior classification (MODR ownership, resolved at asset load) + the MODD
                // colour words; the group's MOLR refs resolve here to WMO-LOCAL light data so
                // the spawn-time fold needs the WMO asset no further.
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
            // Named by the ENTITY as well as the display: the resolver visits each entity once
            // (`WmoPropsResolved` is inserted before any `continue`), so two identical lines mean
            // two instances of one display — two ships of the same model — not a repeat of the
            // work. Without the instance in the message that is unreadable, and it read as a
            // caching bug on 2026-09-06.
            info!(
                "wmo props: {} set-0 doodads resolved for display {} on {entity}",
                props.len(),
                net.display_id.unwrap_or_default()
            );
            commands.entity(entity).insert(WmoProps(props));
        }
    }
}

/// Spawn each pending prop as its M2 lands, parented under the gameobject entity. The whole ship's
/// set spawns within a few frames of the models landing — no per-frame budget (transports are a
/// handful of instances per map, not a city's worth of placements).
pub(super) fn spawn_wmo_gameobject_props(
    mut commands: Commands,
    m2s: Res<Assets<M2Model>>,
    mut forms: ResMut<benilla_world::model_forms::ModelForms>,
    mut mats: benilla_world::model_render::M2BatchMaterials,
    mut uv_reg: ResMut<benilla_world::doodad_anim::UvAnimMaterials>,
    mut tint_reg: ResMut<benilla_world::doodad_anim::TintAnimMaterials>,
    mut anim_table: ResMut<benilla_world::mat_anim_table::MatAnimTable>,
    mut probes: ResMut<PropProbes>,
    // The EXTERIOR prop's one-shot MCSH sample (2047) — the same resolver the terrain lane's
    // placed props use, at the same one-shot cadence the reference's `0x698c50` queue drain has.
    streamer: Option<Res<TerrainStreamer>>,
    adt_tiles: Res<Assets<benilla_assets::AdtTile>>,
    time: Res<Time>,
    mut hosts: Query<(Entity, &GlobalTransform, &mut WmoProps)>,
) {
    let Some((mat_cache, materials, light)) = mats.pieces() else {
        return; // no shared light buffer yet
    };
    let light = &light;
    // The animated-prop clock origin — per-instance phase = spawn time.
    let now = time.elapsed_secs();
    for (entity, host_gt, mut props) in &mut hosts {
        // The host's pose THIS frame — the spawn-time composition base for the interior fold and
        // the emitters' first-frame placement (both track the live pose from here on: the fold is
        // rigid-motion invariant, the emitters follow their owner entity).
        let host_world = host_gt.compute_transform();
        let (mut hulls, mut emitters, mut ribbons, mut lights, mut interior) =
            (0u32, 0u32, 0u32, 0u32, 0u32);
        props.0.retain(|prop| {
            let Some(m) = m2s.get(&prop.handle) else {
                return true; // still loading — retry next frame
            };
            // The prop's app-built render forms: a transport's cabin props are
            // the entity lane — priority 0, same as any creature walking into view.
            let ready = if benilla_world::doodad_anim::wants_rig(m) {
                forms.require_rigged(&prop.handle, 0)
            } else {
                forms.require_static(&prop.handle, 0)
            };
            if !ready {
                return true; // forms still building — retry next frame
            }
            // A boneless prop's glow card follows an ANCHOR child at the prop's placement (the
            // card itself stays a world root the billboard pass writes absolutely). Only spawned
            // when the model has billboard batches — one lantern per ship, not 134 empty entities.
            let card_owner = m.submeshes.iter().any(|s| s.billboard.is_some()).then(|| {
                let anchor = commands.spawn((prop.local, Visibility::default())).id();
                commands.entity(entity).add_child(anchor);
                anchor
            });
            // **An EXTERIOR prop's sun scale is the doodad law, not its host's** (2047). The
            // reference samples MCSH once, at the doodad's OWN footprint, on the frame the
            // pending-doodad queue drains it (`0x698c50` unlinks each entry), and freezes the
            // verdict: 1.0 lit / 0.5 shadowed, never the host node's ramp. `doodad_ground_shade`
            // already answers *lit* for a tile that is not resident — which is what the
            // reference's own null-tile legs return, so a boat at sea needs no special case.
            // An INTERIOR prop ignores the selector entirely (the probe lane reads it not at all).
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
                    // The tile is resident but still decoding — defer this prop a frame, exactly
                    // as the terrain lane does. Not reachable at sea (no tile ⇒ `Ready(false)`).
                    Some(ShadeResolve::Pending) => return true,
                }
            };
            let (radius, center) = m2_fade(&m.bounds, prop.local.scale.x);
            let anim_bound = m2_anim_bound(&m.bounds);
            // The interior lane: fold the cabin prop's committed light ONCE, composed
            // through the host's current pose. Exact on a mover — see the module doc's
            // invariance argument.
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
            // This path never diverts (below), so the identity only names the kind and what the
            // parts are — the GameObject's own `WorldObject` is what a pick on the ship answers
            // with, inserted by `entities::attach` on the parts it owns.
            let object = std::sync::Arc::new(benilla_world::interact::WorldObject {
                kind: benilla_world::model_render::ModelKind::Doodad,
                label: prop
                    .handle
                    .path()
                    .map(|p| p.path().to_string_lossy().into_owned())
                    .unwrap_or_default(),
                id: 0,
                // The prop's LANE, the way a terrain-placed prop's identity names it
                // (`PropLight::inspector_label`): a prop that looks wrong is almost always on the
                // wrong lane or reading the wrong probe, and neither is visible from the model
                // path. This lane's props were the ones the entity shade writer renamed out from
                // under, and a hover said nothing at all.
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
                prop.local, // doodad-LOCAL — the parent composes the world pose
                &object,
                shade,
                interior_slot,
                // No draw-set gate: a transport's props ride a MOVING parent, and an `EmitterFade`
                // measures from a baked world point that a mover has none of — the same reason
                // their emitters carry none (module docs above). The assembler builds the bare
                // sphere its mesh lane needs from `radius`/`center` instead.
                None,
                radius,
                center,
                anim_bound,
                Some((m, now)),
                &mut uv_reg,
                &mut tint_reg,
                &mut anim_table,
                true, // the entity-hosted lane: these props are lit by their own def (2047)
                card_owner,
                // Never diverted into 1417's production merge nor 1429's static-gx: these
                // props parent under a MOVING gameobject, and every divert lane bakes world
                // transforms.
                None,
                None,
            );
            commands.entity(entity).add_children(&ents);
            // The slot frees itself when the prop despawns — the component hook returns it to
            // the table whoever does the despawn (same shape as the terrain path).
            if let (Some(slot), Some(&first)) = (interior_slot, ents.first()) {
                commands.entity(first).insert(PropProbeSlot(slot));
                interior += 1;
            }
            // Solid cargo: collide-iff-hull, the world-placement law. The hull bakes the
            // prop-LOCAL placement into its vertices (the same object-frame the boat's own hull
            // collider uses), and the child carries NO RigidBody — avian attaches a bare collider
            // to the nearest body ancestor, the transport's kinematic body, so the collision
            // sails with the deck. On a static WMO gameobject (Naxxramas) the ancestor is its
            // `Static` body — same mechanism, no motion.
            if let Some((verts, tris)) = placement_collider_data(m.collision.as_ref(), &prop.local)
            {
                // No visibility components at all: a hull renders nothing and hosts nothing,
                // and B0004 only checks the child→parent direction — a bare child under a
                // visible parent is fine (benilla_world::vis_chain has the sweep-tax law).
                let hull = commands
                    .spawn((
                        Transform::IDENTITY,
                        PendingCollider::new(build_collider_task(verts, tris), None, false),
                    ))
                    .id();
                commands.entity(entity).add_child(hull);
                hulls += 1;
            }
            // The prop's live dressing — the entity-owned shape, never the terrain path's
            // world bake; see the module doc. All of it needs a root entity to ride.
            if let Some(&root) = ents.first() {
                let placement = host_world.mul_transform(prop.local);
                for em in &m.emitters {
                    // The emitter rides its host bone's anchor when the prop animates (the
                    // same ride as a terrain doodad — an unanimated chain reproduces the static
                    // path exactly); a boneless prop follows its root. Either owner's propagated
                    // global composes the moving hull.
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
                            // The cloud anchors at the PROP: the boat carries the risen flame (the
                            // reference re-anchors every cloud to the emitter's live position),
                            // while an animated bone still never drags it.
                            anchor: Some(root),
                            // A placed prop's model is destroyed when its placement unloads.
                            on_owner_loss: particles::OwnerLoss::Free,
                            // A placed prop carries no fade component of its own; its emitters
                            // take the doodad distance fade in the sim instead.
                            // A WMO prop is lit by the CMapDoodadDef provider (`0x6a8050`), not the
                            // WENTITY one this edge feeds — its interior words come from its own MODD
                            // colour and are folded at spawn (`PropLight::Interior`). Wiring that twin
                            // into the emitter lane is a named residual, not this lane's node.
                            light_node: None,
                            alpha: None,
                        },
                        // A placed prop: the doodad law — which is NOT one arm for life. It re-rolls
                        // its variation every play-window, so the emitter resolves
                        // the slot + clip time off the host's live player each frame.
                        match host.as_ref().and_then(|h| h.arm) {
                            Some(arm) => particles::EmitClock::Host(arm),
                            None => particles::EmitClock::Pinned,
                        },
                    ) {
                        // Parented: the off-map hide reaches the flame, and despawn cascades
                        // (the sim's direct GlobalTransform write runs post-propagation, so the
                        // parent never disturbs the rendered anchor).
                        commands.entity(entity).add_child(e);
                        emitters += 1;
                    }
                }
                // Ribbon trails — the same host-bone ride; a trail self-despawns with its owner.
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
                        // No model-alpha source: a placed prop / effect instance is always drawn.
                        None,
                        // No fade sphere: a GameObject's props ride the GameObject, not a
                        // baked world placement — that gate's centre is a fixed world point and a
                        // mover would drag away from it (the same reason the emitters beside them
                        // take none, stated at the top of this file), and nothing floods this WMO
                        // to give it rooms. The far-clip wall still applies: `simulate_ribbons`
                        // bounds a fade-less trail at the wall exactly as `simulate_particles`
                        // bounds a fade-less emitter, which is what a map-wide-streamed transport
                        // five kilometres away needs.
                        //
                        // Still open, and named rather than guessed at: a `Visibility`-hidden
                        // transport trails. The trail reads its owner's render alpha, not its
                        // `Visibility`, and the off-map hide writes the latter. No 1.12 transport
                        // prop authors a ribbon, so nothing renders it today.
                        None,
                    )
                    .is_some()
                    {
                        ribbons += 1;
                    }
                }
                // M2 point lights (the lantern's glow source): a CHILD at the prop-local
                // position — propagation carries the source with the hull.
                for l in m.lights.iter().map(|l| &l.def) {
                    if !l.casts() {
                        continue; // directional lights feed an ambient term; a static `0` visibility key is dark
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
            false // spawned — drop from the pending list
        });
        if hulls + emitters + ribbons + lights + interior > 0 {
            // The instrument line the probe greps: what this host's props actually authored
            // (zero anywhere is legal — collide-iff-hull, emit-iff-authored, indoor-iff-owned).
            // The host is in the message for the same reason as above — and doubly here, because
            // this line fires once per host per frame in which anything spawned, so ONE host's
            // 134 props land over several batches and emit several honest lines.
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
