//! Spawning streamed placements once their model assets land: the per-frame budgeted
//! [`spawn_loaded_placements`] driver + WMO doodad-prop resolution and the small per-placement
//! helpers (inspector tags, fade spheres). The submesh/material/fade component assembly lives in
//! [`assemble`] ([`spawn_model_entities`]), the per-model point lights/emitters/ribbons in [`fx`].

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

/// Placements (models or WMO props) spawned per frame while live — the count half of the landing
/// budget; see the cap's comment in [`spawn_loaded_placements`]. ~150 × the measured ~50 µs
/// downstream cost per landed placement ≈ half a 60 Hz frame of render-side work, and a fresh
/// Undercity row (~3900 placements) fully furnishes in under half a second, two tiles out.
const SPAWN_COUNT_CAP: usize = 150;

/// Spawn the submesh entities for any placement whose model asset has finished loading. Each submesh
/// gets the production [`WowModelMaterial`] (cutout + blend twin) and the same `ModelPart`/`DoodadFade`/
/// `MeshTag` components the old spawn used — so the existing visibility/fade/lighting systems take over.
/// The nested resource tuple of [`spawn_loaded_placements`] (the 16-SystemParam ceiling): the
/// prop-probe table, the stream-trace counters, the live/settling state the landing cap reads,
/// and the model-forms cache. The skin-palette table left with decision 0863 — a
/// placement's rig slot is the draw gate's to claim at first wake now, not the spawner's.
type SpawnTables<'w> = (
    ResMut<'w, PropProbes>,
    ResMut<'w, crate::terrain_stream::StreamActivity>,
    Res<'w, crate::terrain_stream::ViewFocus>,
    ResMut<'w, ModelForms>,
    ResMut<'w, HullWelds>,
    ResMut<'w, StaticMerge>,
    // `None` only under `WOW_STATIC_GX=0` (the resource exists whenever the retained pass
    // is armed — the default since 1434).
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
    // The shared per-kind animated liquid materials — WMO groups spawn their embedded water/lava/slime
    // on these, same as MCLQ terrain water. Absent when the client has no data.
    liquid_assets: Option<Res<LiquidAssets>>,
    // Read-only: the loaded-tile map + decoded tiles, for the global MCSH ground-shade lookup at spawn.
    // `stream_terrain` (chained before this) owns them mutably; here we only read.
    streamer: Res<TerrainStreamer>,
    adt_tiles: Res<Assets<AdtTile>>,
    time: Res<Time>,
    mut uv_reg: ResMut<crate::doodad_anim::UvAnimMaterials>,
    mut tint_reg: ResMut<crate::doodad_anim::TintAnimMaterials>,
    mut anim_table: ResMut<crate::mat_anim_table::MatAnimTable>,
    // Nested to stay inside Bevy's 16-element system-param tuple limit: the prop-probe table +
    // the stream-trace counters + the live/settling state the landing cap reads + the
    // model-forms cache.
    tables: SpawnTables,
) {
    let (mut probes, mut activity, focus, mut forms, mut welds, mut merge, mut staticgx) = tables;
    let Some(shared_light) = shared_light else {
        return;
    };
    // Steady state is everything spawned, and the walk below still visits every placement and
    // every WMO prop to find that out — the pending count (kept by the register/handoff/release
    // sites) makes that frame free.
    if placements.pending_spawns == 0 {
        return;
    }
    let t0 = Instant::now();
    // The landing COUNT cap, on top of the time budget below. The clock gates main-thread
    // cost — but ~1000 one-submesh doodads pass a 4 ms budget in 2.5 ms and hand the render world
    // their whole extract/specialize/upload wave in one frame: the measured 60–80 ms landing
    // frame when a fresh Undercity row streams in. Counting bounds the downstream wave the clock
    // cannot see. Idle until the body is live-and-settled — the entry/teleport cover exists to
    // absorb exactly this burst, and capping under it would only lengthen the reveal.
    let count_cap = if focus.paced {
        SPAWN_COUNT_CAP
    } else {
        usize::MAX
    };
    let mut spawned_n = 0usize;
    // The animated-doodad clock origin: per-instance phase = spawn time, as in the
    // reference (the arm-time cursor offset), and the draw gate seeks against it on resume.
    let now = time.elapsed_secs();
    let light = &shared_light.0;
    let materials = materials.into_inner();
    let Placements {
        by_id,
        materials: mat_cache,
        pending_spawns,
    } = placements.into_inner();

    // Per-frame spawn budget. On cold start every tile's doodads/WMOs finish decoding near-together;
    // spawning them all in one frame builds thousands of parry trimesh colliders synchronously and
    // freezes the window for seconds. Spend at most `SPAWN_BUDGET` per frame and resume next frame
    // (the `spawned` flags make this re-entrant) — a self-tuning cap: cheap doodads spawn many per
    // frame, an expensive WMO collider one. At least one spawn always happens (the check is after the
    // work), so progress is guaranteed.
    let deadline = Instant::now() + SPAWN_BUDGET;

    'placements: for (&unique_id, p) in by_id.iter_mut() {
        // 1. Spawn the model's own geometry once, on first load. For a WMO, also resolve its doodad
        //    props (`p.doodads`) — spawned individually in step 2 as each prop's M2 asset arrives.
        if !p.spawned {
            let entities = match &p.model {
                ModelHandle::M2(h) => {
                    let Some(m) = m2s.get(h) else {
                        continue; // model still loading (or missing) — try next frame
                    };
                    // The model's app-built render forms: request static — plus
                    // the skinned twins iff the anim host will rig this model — and wait for the
                    // paced furnisher, exactly as the placement waits for the asset itself.
                    let key = ModelKey::from(h);
                    let kinds = WANT_STATIC | if wants_rig(m) { WANT_SKINNED } else { 0 };
                    if !forms.require(
                        key,
                        kinds,
                        placement_priority(&streamer, p.transform.translation),
                    ) {
                        continue;
                    }
                    // Resolve the MCSH ground-shade the reference way: a GLOBAL world→tile→chunk lookup at
                    // the doodad's origin, independent of which tile registered it or in what order.
                    //
                    // **An ADT map doodad is the FIXED-1.0 family, not the boosted one** (2050).
                    // This site read `ShadeSel::Lit` (the 2.5 target), which was wrong: a
                    // `CMapDoodadDef`'s `[+0xa4]` is only ever {0.0, 0.5, 1.0}, and the class does
                    // not merely fail to ramp — at `+0xf8`, where the ramp `0x69e770` reads its
                    // target, the doodad class holds `m[2][3]` of its own world matrix. The 2.5 and
                    // the 3.3333/s chase belong to the WENTITY light node (vtable `0x810810`, hung
                    // off `[obj+0xe0]`), which a doodad has none of — the 2.5 constant's one reader
                    // in the image is `0x69e4ad`. Measured as well as read: 111 identified MDDF
                    // placements over six trace frames, 356 draws, gains only ∈ {0.5, 1.0}, none
                    // changing — while an entity walking out of shadow climbed 0.5 → 0.81 → 1.09 →
                    // 1.3367 in the same frames.
                    //
                    // So this is the same family the exterior WMO MODD prop takes, and for the same
                    // reason: they are one C++ class. Nothing renders differently today — the
                    // shader's `min(I, 1)` already flattened the two — but the scale being
                    // per-class is what has to be true BEFORE that cap can come off.
                    let shade =
                        match doodad_ground_shade(&streamer, &adt_tiles, p.transform.translation) {
                            ShadeResolve::Ready(true) => ShadeSel::Shaded,
                            ShadeResolve::Ready(false) => ShadeSel::Matte,
                            // The doodad's own ground tile is requested but still decoding — wait, so we don't
                            // bake the lit fallback into a straddling tree whose true tile lands a frame later.
                            ShadeResolve::Pending => continue,
                        };
                    let (radius, center) = m2_fade(&m.bounds, p.transform.scale.x);
                    let anim_bound = m2_anim_bound(&m.bounds);
                    // The placement's ONE draw-set gate, built before the spawn because the
                    // assembler now takes it too (2059): the mesh lane, the emitters, the ribbons
                    // and the anim host are admitted or refused together, from one expression. A
                    // map doodad is nobody's furniture, so it carries no instance and no rooms.
                    let fade = emitter_fade(p.transform, (radius, center), None, None);
                    // The placement's identity, built BEFORE the spawn: every lane that takes
                    // a batch — entity, merge blob, retained cell — carries this same Arc, so
                    // whichever one draws it, the pick names the placement.
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
                        // The retained-pass collector (1429/1431) — ADT doodads are its lane.
                        staticgx
                            .as_deref_mut()
                            .map(|gx| (gx, crate::static_gx::GxSite::Doodad { owner: p.owner })),
                    );
                    // ADT doodads are exterior scene: from inside a WMO they draw only through a
                    // portal window (`0x683700`, fed solely by the per-window walk `0x682fa0` — see
                    // `crate::exterior_cull`). Tagged per submesh because a placement has no root
                    // entity to carry the bound; the deviation from the reference's whole-object test
                    // is recorded in `exterior_cull`'s module doc.
                    //
                    // The anim-host ROOT is skipped: it is a joint hierarchy, not geometry, so it has
                    // no `Aabb` and lands in the cull's fail-open arm — harmless to draw (it draws
                    // nothing) but it was 484 of the 6917 "tested" objects at one Stratholme pin,
                    // which is exactly the kind of noise that makes the instrument stop meaning
                    // anything. Tag what is drawn.
                    let anim_root = host.as_ref().map(|h| h.root);
                    for e in ents.iter().filter(|e| Some(**e) != anim_root) {
                        commands
                            .entity(*e)
                            .insert(crate::exterior_cull::ExteriorScene);
                    }
                    // Doodad collider (avian): a static trimesh hull baked at this placement's
                    // transform, WELDED into its owner tile's batch (the entity
                    // granularity was ~0.8 cpu_ms of avian per-frame cost at Stormwind; see
                    // `super::weld`). `None` ⇒ the model has no collision hull, so a hull-less
                    // tree canopy stays pick-through, matching the reference's
                    // collision-flagged-doodads-only world trace — the weld carries the
                    // `PickOccluder` clamp for the hulls it absorbs.
                    // (`!doodad_bodies_disabled()` / `!doodad_hulls_bare()`: the 1367 premise
                    // levers. `hull_weld_disabled`: the pre-1369 per-placement shape, for A/B.)
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
                    // One gate for both emitter families. `None`/empty: an ADT map doodad
                    // belongs to no building, so neither the window exemption nor the room
                    // term has anything to key on.
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
                    // The building's app-built render forms: static only — WMO group
                    // geometry never skins. A city root's thousands of batches are exactly the
                    // burst the paced furnisher exists to spread.
                    let key = ModelKey::from(h);
                    let prio = placement_priority(&streamer, p.transform.translation);
                    if !forms.require(key, WANT_STATIC, prio) {
                        continue;
                    }
                    // WMOs carry no authored bounds → never size-fade (∞ radius), rely on the far-clip.
                    // (`m2: None` ⇒ no anim host, so the joint half of the return is always empty.)
                    // Hoisted above the spawn: the merge site needs it (a portal-gated building's
                    // blobs take `WmoGroupVis`), and the instance logic below reuses it.
                    let has_portals = !m.portal_refs.is_empty() && !m.portal_infos.is_empty();
                    // A building carries no authored M2 bounds, so its gate is the never-fade
                    // sphere the `f32::INFINITY` radius below always meant. `m2: None` on this
                    // lane ⇒ no anim host, so only the mesh half of the gate is ever read.
                    let fade = emitter_fade(p.transform, (f32::INFINITY, Vec3::ZERO), None, None);
                    // The building's identity, shared by every lane that takes one of its batches
                    // — see the doodad site above.
                    let object = Arc::new(WorldObject {
                        kind: ModelKind::Wmo,
                        label: handle_label(h),
                        id: unique_id,
                        detail: String::new(),
                    });
                    // The per-placement portal-cull instance, spawned BEFORE the batches so the
                    // static-gx divert can key its retained region on it (slice 2 of 1429 — the
                    // region's lifecycle IS this entity's). The group-vis tagging still runs
                    // after the spawn, off `by_batch`; see the instance block below for why a
                    // portal-less-but-named building spawns one too.
                    let instance = (has_portals || m.wmo_id != 0).then(|| {
                        commands
                            .spawn(WmoPortalInstance {
                                handle: h.clone(),
                                world_from_local: p.transform.compute_affine(),
                                name_set: p.name_set,
                                visible: vec![true; m.group_nav.len()],
                                // Nothing wears the building's own fog until a flood says
                                // it is on the chain (`WmoPortalInstance::interior_fog`).
                                interior_fog: vec![false; m.group_nav.len()],
                                liquid_visited: vec![false; m.group_nav.len()],
                                // The MOGP `groupLiquid` override, resolved once at spawn: 13
                                // groups in the whole archive declare "this room is wholly
                                // submerged" in place of carrying a liquid grid, and 5 of them are
                                // placed — the Deeprun Tram's two flooded sections, the Prison
                                // Oubliette, the MD crypt and the mountain cave.
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
                        ShadeSel::Matte, // WMO lights on the FFP N·L path — the selector is unread
                        None, // WMO groups carry their own per-submesh interior flag + batch class
                        Some(&fade),
                        f32::INFINITY,
                        Vec3::ZERO,
                        None, // …and no authored M2 box: group geometry never animates
                        None, // WMO group geometry is not an M2 — its doodad props animate below
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
                        // Slice 2 (1429): WMO group geometry diverts into a retained region
                        // keyed by the instance entity — only a placement WITH an instance
                        // qualifies (no instance ⇒ no PVS identity ⇒ entity path).
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
                    // A world WMO placement is exterior scene too (`0x6856c0`, fed by the same
                    // per-window populate `0x682fa0`): from inside one building, another building
                    // draws only through a portal window. 0774 left this ungated because these
                    // entities already had a `Visibility` authority and a second writer would have
                    // fought it — that is fixed at the root now, with the window term folded INTO
                    // that authority, so the tag is safe to add.
                    //
                    // Tagging is unconditional and the **exemption is dynamic**: the camera's own
                    // containing placement is not exterior to itself, which the authority decides
                    // per frame from `CameraInteriorClaim`. A static "is this the player's building"
                    // could not be right — the player walks in and out of it.
                    for e in &ents {
                        commands
                            .entity(*e)
                            .insert(crate::exterior_cull::ExteriorScene);
                    }
                    // Portal visibility + interior tracking: tie every group submesh entity (the
                    // `by_batch` map is index-parallel with the submeshes — the structural form of
                    // what used to be a positional promise on `ents`, which the mega divert's
                    // `continue` silently broke) back to one per-placement instance that holds the
                    // placement transform + the per-frame visible set. The cull is per-group, so a building with a portal graph
                    // gets tagged; a portal-less prop keeps all groups visible but still spawns the
                    // instance when it carries a WMOAreaTable identity — the interior down-ray
                    // (`wmo_portal::track_current_interior`) needs it. See `crate::wmo_portal`.
                    if let Some(instance) = instance {
                        if has_portals {
                            // One shared single-group key per group — geometry belongs to exactly
                            // one room, and sharing keeps the tag one allocation per group rather
                            // than per submesh (a city is ~100k submeshes).
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
                        // The props spawn later — each waits on its own M2 — so they can't be
                        // tagged here; hold the instance for them. Held for EVERY
                        // instance, not just a portal-bearing one: since 0696 it is also the
                        // placement identity each embedded pool is scoped by, and a portal-less
                        // building's pool needs an owner exactly as much as a canal does.
                        p.portal_instance = Some(instance);
                        ents.push(instance); // despawns with the placement
                    }
                    // The building's embedded MLIQ liquid (Stormwind's canals + fountains, the
                    // Ironforge lava, dungeon pools): one flat animated surface per group with water,
                    // spawned at the placement transform on the shared per-kind material. (The group
                    // zip above reads `by_batch`, so appends to `ents` no longer threaten it.)
                    // Spawned a group at a time so each surface can take that group's cull key: a
                    // pool belongs to the room it sits in, and a culled room's lava must go with it
                    // (the same defect as the props, on the same building).
                    for (gi, lq) in m.group_liquids.iter().enumerate() {
                        let Some(lq) = lq else { continue };
                        let first = ents.len();
                        spawn_wmo_liquids(
                            &mut commands,
                            std::iter::once(lq),
                            liquid_assets.as_deref(),
                            &mut meshes,
                            p.transform,
                            // The group's own interior class (`MOGI & 0x48 == 0`) — which FOG BLOCK
                            // its pool draws under, so an indoor pool hazes with the room the way
                            // the walls beside it do (decision 0691's open lane).
                            m.group_bounds.get(gi).is_some_and(|g| g.interior),
                            // …and the pool's SCOPE: the room it belongs to (only a subject
                            // standing in this placement can be in it) plus that room's own floor
                            // in world Z (and only one at or above it).
                            crate::liquid::WmoPool::new(
                                p.portal_instance.map(|instance| WmoRoom {
                                    instance,
                                    group: gi as u16,
                                }),
                                &p.transform,
                                m.group_bounds.get(gi),
                            ),
                            // The root's MOMT diffColor table — an interior pool's body colour.
                            &m.material_diff_color,
                            &mut ents,
                        );
                        // Another building's canal is exterior scene like its walls (`0x6856c0`,
                        // the same per-window populate). **Unconditional, like the walls at the
                        // `ExteriorScene` site above** — decision 1652's second half, and a
                        // structural fix with **no shipped symptom**, which is the honest framing.
                        //
                        // It used to be tagged only *inside* the `portal_instance` arm below,
                        // because `WmoGroupVis` carries the instance the camera's-own-building
                        // exemption is keyed on. That left a pool with NEITHER component — and so
                        // no `Visibility` authority at all — whenever a placement got no instance
                        // (`has_portals || m.wmo_id != 0`, ~line 334), while the same placement's
                        // walls, tagged unconditionally, were window-culled normally.
                        //
                        // **That branch is dead on shipped 1.12.1 content** (`cargo run -p
                        // benilla-formats --example wmo_ownerless_pools`): of 815 roots, 448 have
                        // no portal graph and exactly ONE has `wmoID == 0` — and that one
                        // (`pvp_alterac_ent01.wmo`) *does* carry portals. The two conditions never
                        // coincide, so zero placements reach the hole and none of them carries
                        // liquid. Kept anyway because it costs nothing and the asymmetry with the
                        // walls two blocks up was a real defect in the code even where the data
                        // never exercised it — but it must never be credited with a sighting.
                        //
                        // An instance-less placement can never BE the camera's claimed room (the
                        // interior down-ray resolves through an instance), so there is no
                        // exemption to lose: the tag alone is right for it, and
                        // `apply_exterior_cull` is its sole authority exactly as for ADT liquid.
                        for &e in &ents[first..] {
                            commands
                                .entity(e)
                                .insert(crate::exterior_cull::ExteriorScene);
                        }
                        if let Some(instance) = p.portal_instance {
                            let groups: Arc<[u16]> = Arc::from([gi as u16].as_slice());
                            for &e in &ents[first..] {
                                // …and where there IS an instance, the pool rides its group with
                                // the room and takes the exemption — which moves it
                                // out of `apply_exterior_cull`'s query (`Without<WmoGroupVis>`)
                                // and into the model-visibility authority, the one that can see
                                // both terms.
                                commands.entity(e).insert(WmoGroupVis {
                                    instance,
                                    groups: groups.clone(),
                                });
                            }
                        }
                    }
                    // Building colliders (avian): the flattened group tris baked at this placement's
                    // transform — *two* meshes, because the client gathers different WMO faces for the
                    // player body (walking: drops DETAIL) and the camera/LOS (drops NOCAMCOLLIDE, keeps
                    // DETAIL). Each goes on its own collision layer so the player and camera queries pick
                    // their audience (see `crate::collision`); a single trimesh can't be filtered per-face.
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
                                    // WMO walkable faces receive the selection ring (floors, steps).
                                    GroundDecalSurface,
                                    // …and clamp the mouse pick: the byte-verified occluder face
                                    // set is MOPY reject-mask 0x84, and the walk bake (reject 0x04)
                                    // is its nearest existing bake — see `crate::collision`.
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
                    // Interior MOLT lights (forge fire, inn fireplaces, chapel candles) — the radiating
                    // sources that light nearby NPCs/doodads AND the building's own walls/floor over
                    // their baked MOCV.
                    spawn_wmo_lights_for(
                        &mut commands,
                        &m.lights,
                        &m.group_light_refs,
                        p.portal_instance,
                        p.transform,
                        &mut ents,
                    );
                    tag_world_object(&mut commands, &ents, &object);
                    // Resolve the interior props for this instance's doodad set (set 0 + the selected
                    // set), each composed onto the WMO's world transform; their M2s load async.
                    p.doodads = resolve_wmo_doodads(m, p.doodad_set, p.transform, &asset_server);
                    ents
                }
            };
            p.spawned = true;
            p.entities = entities;
            // The placement's own model landed; a WMO's just-resolved props (all unspawned)
            // are new work the count takes on in the same stroke.
            *pending_spawns += p.doodads.iter().filter(|d| !d.spawned).count();
            *pending_spawns -= 1;
            activity.placements_spawned += 1;
            spawned_n += 1;
            // Spent the frame's spawn budget on this model (a WMO with two collider meshes is the
            // costly case) — defer the rest, including this placement's WMO props, to next frame.
            if Instant::now() >= deadline || spawned_n >= count_cap {
                break 'placements;
            }
        }

        // 2. Spawn each WMO doodad prop as its M2 asset finishes loading (across frames). Their
        //    entities join `p.entities`, so they despawn with the placement. Empty for M2 doodads.
        // Copied out before the loop borrows `p.doodads`: the props tag onto the same portal
        // instance their building's groups did.
        let portal_instance = p.portal_instance;
        for d in &mut p.doodads {
            if d.spawned {
                continue;
            }
            let Some(m) = m2s.get(&d.handle) else {
                continue; // this prop's M2 still loading
            };
            // The prop's app-built render forms — same gate as its owning placement's.
            let key = ModelKey::from(&d.handle);
            let kinds = WANT_STATIC | if wants_rig(m) { WANT_SKINNED } else { 0 };
            if !forms.require(
                key,
                kinds,
                placement_priority(&streamer, d.transform.translation),
            ) {
                continue;
            }
            // The verified MODD lighting: an EXTERIOR-group WMO prop samples the terrain MCSH at
            // its footprint like the reference's per-frame refresh `0x698c50` — plain matte (sun
            // ×1.0) on lit ground, the shaded level on MCSH-shadowed ground, and NEVER the ADT 2.5
            // boost (its one read site, `0x69e4ad`, is unreachable from the WMO render band). An
            // INTERIOR prop takes its per-instance SH probe instead — the selector is unread there.
            let shade = if matches!(d.light, PropLight::Interior { .. }) {
                ShadeSel::Matte // interior lane; the selector is unread
            } else {
                match doodad_ground_shade(&streamer, &adt_tiles, d.transform.translation) {
                    ShadeResolve::Ready(true) => ShadeSel::Shaded,
                    ShadeResolve::Ready(false) => ShadeSel::Matte,
                    // The prop's ground tile is still decoding — defer this prop a frame (same rule
                    // as an ADT doodad; the per-prop `spawned` flag makes it re-entrant).
                    ShadeResolve::Pending => continue,
                }
            };
            let (radius, center) = m2_fade(&m.bounds, d.transform.scale.x);
            let anim_bound = m2_anim_bound(&m.bounds);
            // One gate for every rider of this prop: the prop rides its building's
            // exterior-window exemption AND the portal PVS of the rooms that name it — the same
            // `WmoGroupVis` the submeshes are culled by, so a prop's mesh, its flames, its
            // streamers and (since 2059) its ANIM HOST are admitted or refused together.
            // Hoisted above the spawn because the assembler takes
            // it now; a particles-only prop has no submesh to carry a verdict, so this is the only
            // thing that tells its host it is furniture rather than exterior scene.
            let fade = emitter_fade(
                d.transform,
                (radius, center),
                portal_instance,
                Some(&d.groups),
            );
            // An interior prop's committed light, folded ONCE into its SH probe (the reference folds
            // at doodad create + per-frame light commit; everything here is static, so once): the
            // MODD-colour ambient + the diffuse lobe on the fixed interior axis + the owning group's
            // MOLR lights windowed by their disk attenStart/attenEnd from the prop's REFERENCE POINT
            // — the placement-transformed M2 vertex-box centre (the byte-cited `[def+0x5c]` anchor
            // family `0x6952a0`/`0x713680`; the trace fit put it ~1.1 yd up on the abbey stand,
            // between origin and centre — the centre satisfies every observed gain, residual OPEN).
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
                        // Once per burst, not per prop — a city WMO can flood thousands in a frame.
                        let (live, peak) = probes.occupancy();
                        warn_once!(
                            "interior-prop probe table full (live {live}, peak {peak});                              overflowing props fall back to exterior light"
                        );
                    }
                    slot
                }
            };
            // The prop's identity, shared with whichever lane takes its batches (1534).
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
                // The prop merge site (1418 lane 3): keyed by the rooms that name the prop,
                // slot baked per vertex for the interior lane.
                Some((
                    &mut *merge,
                    MergeSite::Prop {
                        uid: unique_id,
                        groups: &d.groups,
                        slot: interior_slot,
                    },
                )),
                // The prop retained-pass site (B4): region keyed by the
                // building's instance entity — the PVS identity AND the lifecycle. No
                // instance ⇒ no region key ⇒ the merge/entity path, tallied per prop so
                // the declined population is never silent.
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
            // The slot frees itself when the prop despawns (streaming out) — the component hook
            // returns it to the table whoever does the despawn. A prop whose every batch
            // diverted into a merge blob (1418 lane 3) spawns a bare carrier row for the hook:
            // the blob aggregates many props and cannot own any single slot's lifetime.
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
            // Portal-cull the prop with the rooms that name it — the reference commits a WMO's
            // doodads per VISIBLE group (`0x695aa0` over the group's MODR refs, from the
            // visible-group walk `0x698720`), so furniture never outlives its room
            // and a prop several rooms name is drawn from ANY of them. Every submesh spawned above
            // is this one prop, so they all share the one key.
            //
            // The anim-host ROOT is skipped, for the reason the ADT site skips it: it is a
            // joint hierarchy, not geometry — no `ModelPart`, no `Aabb` — so it lands in the cull's
            // fail-open arm and inflates every "objects tested" count without ever being drawn. The
            // ADT lane has excluded it since 0784; this lane did not, which made the same root a
            // silent passenger of the WMO prop path's counts.
            let anim_root = host.as_ref().map(|h| h.root);
            if let (Some(instance), false) = (portal_instance, d.groups.is_empty()) {
                for &entity in ents.iter().filter(|e| Some(**e) != anim_root) {
                    commands.entity(entity).insert((
                        WmoGroupVis {
                            instance,
                            groups: d.groups.clone(),
                        },
                        // Furniture is exterior scene when its building is — and exempt when
                        // that building is the one the camera stands in. Keyed on the instance
                        // `WmoGroupVis` carries, so a prop no group names (no key, no exemption
                        // possible) is deliberately left untagged rather than gated blind.
                        crate::exterior_cull::ExteriorScene,
                    ));
                }
            }
            // Prop collider (avian): a static trimesh from the prop's collision hull at its world
            // transform — collide-iff-hull, exactly like a map doodad, so a weapon rack / crate / cargo
            // net is solid to both the player and the camera while a hull-less prop (banner, small
            // candle) isn't. Default collision layer ⇒ both audiences. WELDED per placement:
            // every prop of a building despawns with the building, so the batch
            // has exactly the lifetime the individual hulls had — the weld flushes into
            // `Placement::entities` one chain-step later. (The 1367 premise levers and the 1369
            // A/B lever gate here exactly as at the map-doodad site above.)
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
                                // A world-static WMO prop's hull clamps the mouse pick like any doodad.
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
                fade.room.as_ref(), // the prop's glow rides its rooms like its mesh (0689)
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

/// Resolve a WMO instance's visible doodad props: the global set (0, always shown) plus the
/// placement's MODF-selected set, each prop composed onto the WMO's world transform and given its M2
/// handle (loaded async, spawned by [`spawn_loaded_placements`] when ready). Props with an unresolved
/// name are skipped.
fn resolve_wmo_doodads(
    wmo: &WmoModel,
    doodad_set: u16,
    wmo_world: Transform,
    asset_server: &AssetServer,
) -> Vec<WmoDoodadInst> {
    // Set 0 is the always-shown global set; a non-zero placement set adds one more.
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
            // Interior classification (MODR ownership) + the MODD-colour base + the owning group's
            // MOLR refs were all resolved ONCE at asset load (`WmoModel::doodad_base`); here the
            // placement composes the referenced lights into WORLD space so the spawn-time fold
            // never needs the WMO asset again.
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
                // Every MODR referrer, inverted with the lighting base at load — the prop's
                // portal-cull key, so a lantern is hidden with the room it hangs in
                // and a lava fall the rooms below and above both name survives either being culled.
                groups: wmo.doodad_groups.get(di).cloned().unwrap_or_default(),
                light,
                spawned: false,
            });
        }
    }
    out
}

/// `WOW_FADE_NEAR="x,y,r"` — the "name that popping doodad" instrument: a WoW server-coordinate
/// point (the same numbers `.go xyz` takes, director-verbatim) plus a horizontal radius in yards.
/// Every placement spawning within `r` yd of the point logs its model path, fade radius (which
/// selects the `model_fade` band), scale, and how many of its batches the static merge diverted —
/// so a "this doodad pops in at .go X Y Z" report converts into the placement's actual fade
/// inputs and its lane (merged fader / per-entity / WMO) in one parked probe run, no camera work.
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

/// One `[fade-near]` line for a placement inside the [`fade_near_target`] circle — shared by the
/// doodad and WMO arms so the two print comparably. `radius` is the fade input (∞ for a WMO);
/// `diverted` of `batches` went to the static merge (the lane readout).
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

/// A model handle's source path as a readable label for the object inspector (the asset path without
/// the `mpq://` source prefix). Empty if the handle carries no path.
/// A placement's model-forms build priority: its Chebyshev tile distance to the
/// stream focus, scaled to leave the band below for the entity lane — a mob walking into view
/// never queues behind a city's scenery. `translation` is Bevy space (the placement transform);
/// `world_to_tile` wants WoW ground coords, the inverse of decision 0002's rotation.
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

/// Tag every spawned entity of a placement with its [`WorldObject`] identity (so the mouseover
/// inspector can name it). Harmless on the non-mesh entities (colliders) — only mesh entities are
/// ray-picked.
///
/// The identity is built by the caller and passed IN, because a batch that diverts into a
/// consolidating lane spawns no entity for this to reach: the same Arc rides the divert as the
/// lane's pick-member identity. This tag covers what the entity path did spawn —
/// including the fx entities (emitters, ribbons, lights) that no lane ever takes.
fn tag_world_object(commands: &mut Commands, ents: &[Entity], object: &Arc<WorldObject>) {
    for &e in ents {
        commands.entity(e).insert((**object).clone());
    }
}

/// The **animated cull bound** for an M2 placement: the model's authored header bounding box
/// (`M2Header.bounding_box_min/max`), in Bevy model-local space. `None` when the model carries no
/// authored bounds.
///
/// This box is the model's **all-animation vertex extent**, not its bind pose — which is exactly why
/// it is the bound an *animated* placement must be culled with. A bind-pose mesh bound is the extent
/// of geometry the entity transform no longer describes: the joint palette moves the vertices while
/// the entity stays parked at the placement origin, so the cull tests an empty box. `World\critter\
/// birds\Bird01.m2` is the extreme: a 1.2 × 1.8 × 0.23 yd bind-pose box, and a root-bone translation
/// track that flies the bird 64 yd along X and 17 yd along Y away from it. Culled by the bind pose,
/// the bird blinks out whenever that 1-yd box leaves the frustum while the bird itself is still on
/// screen — the director's "birds in the sky often dis/appear based on the cam angle".
///
/// The reference tests one volume per doodad *object* and derives it from these same header fields:
/// `0x683700` calls `0x682ef0(ecx = &[rec+0x5c] centre, [rec+0x68] radius)` → `0x686b80`, a 6-plane
/// frustum **sphere** test, where `rec+0x5c` is the transformed `(min+max)/2` and `rec+0x68` is
/// `bounding_sphere_radius × scale` (the same two fields [`m2_fade`] already reads). We keep the
/// **box** rather than its circumsphere: it bounds the same geometry more tightly, and Bevy's cull
/// is an OBB test anyway.
pub fn m2_anim_bound(bounds: &Option<M2Bounds>) -> Option<Aabb> {
    let b = bounds.as_ref()?;
    // The basis swap permutes and negates axes, so min/max have to be re-derived, not mapped.
    let (a, c) = (wow_to_bevy(b.bbox_min), wow_to_bevy(b.bbox_max));
    let (lo, hi) = (a.min(c), a.max(c));
    // A model that authors a degenerate (all-zero) box would otherwise pin every submesh of an
    // animated placement to a point at the origin — strictly worse than the bind pose it replaces.
    (hi.cmpgt(lo).any()).then(|| Aabb::from_min_max(lo, hi))
}

/// The distance-fade size for an M2: world radius = authored bounding-sphere radius × placement scale;
/// centre = authored bbox centre in Bevy model-local space. Absent bounds ⇒ never size-fade.
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
