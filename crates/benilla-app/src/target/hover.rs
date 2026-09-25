//! The mouseover picks, each frame: the unit under the cursor into [`super::Hovered`] and the
//! GameObject into [`super::HoveredObject`], as the reference's pick (`0x481190` → `0x7089c0`)
//! finds them.

use std::collections::HashSet;

use benilla_assets::ModelAnimations;
use benilla_protocol::EntityKind;
use bevy::camera::primitives::Aabb;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::creature_anim::AnimDriver;
use crate::entities::HeldAttached;
use crate::net::{Guid, NetEntity, ObjectStore, SelfPlayer};
use crate::player::CameraControl;
use crate::ui_script::PointerOverUi;
use benilla_world::billboard::BillboardCard;
use benilla_world::collision::PickOccluder;
use benilla_world::interact::{
    ray_mesh_bounds, ray_posed_mesh, CreaturePickPart, GoPickPart, PickParts,
};
use benilla_world::view::WorldCamera;

use super::{Hovered, HoveredObject, PickOcclusion};

/// Trace this frame's world hit, the reference's `CWorld::Intersect` leg of the scene trace
/// (`0x480df0`), through the [`PickOccluder`] set: terrain, the WMO walk-bake faces (about the
/// `0x84` reject mask) and static doodad hulls. Net entities are the object trace, not occluders,
/// so a chest never occludes itself. The picks run unbounded and compare against it after.
pub(super) fn update_pick_occlusion(
    camera: Query<(&Camera, &GlobalTransform), With<WorldCamera>>,
    window: Query<&Window, With<PrimaryWindow>>,
    spatial: avian3d::prelude::SpatialQuery,
    occluders: Query<(), With<PickOccluder>>,
    mut occlusion: ResMut<PickOcclusion>,
) {
    occlusion.distance = f32::INFINITY;
    occlusion.point = None;
    let (Ok((camera, cam_tf)), Ok(window)) = (camera.single(), window.single()) else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let Ok(ray) = camera.viewport_to_world(cam_tf, cursor) else {
        return;
    };
    if let Some(hit) = spatial.cast_ray_predicate(
        ray.origin,
        ray.direction,
        f32::MAX,
        true,
        // The mask that reaches every `PickOccluder` collider; the marker picks the set within it.
        &benilla_world::collision::WorldCollision::body_filter(),
        &|e| occluders.contains(e),
    ) {
        occlusion.distance = hit.distance;
        occlusion.point = Some(ray.origin + *ray.direction * hit.distance);
    }
}

/// The model instances whose parts are `unit`'s pick geometry: the body, everything it wears, and
/// its mount. The reference's `0x480d90` walks the CM2 attachment tree (`[model+0x1dc]` head,
/// `[+0x1e4]` sibling) and registers each model (`0x713cb0`) under one candidate, so a hit on a
/// held axe is a hit on its wielder. `pub(crate)` for the capture census, which counts the same.
pub(crate) fn pick_model_roots(
    unit: Entity,
    worn: &[Option<Entity>],
    mount: Option<Entity>,
) -> impl Iterator<Item = Entity> + use<'_> {
    std::iter::once(unit)
        .chain(worn.iter().flatten().copied())
        .chain(mount)
}

/// Every model's current skeleton pose, as one [`SystemParam`] (the picker is at Bevy's 16-param
/// limit): the palette rows and the per-part rig link that indexes them.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct PickPose<'w, 's> {
    /// The world-space matrices the vertex stage skins with, read from the CPU rows.
    palettes: Res<'w, benilla_world::rig_palette::RigPalettes>,
    rigs: Query<'w, 's, &'static benilla_world::rig_palette::RigSkin>,
}

/// The unit under the cursor, as the reference's pick (`0x7089c0`) finds it. Broad phase: the ray
/// against the playing sequence's bounds sphere. Pass 1: the ray against the posed render mesh,
/// nearest wins. Pass 2, the mouse pick's retry when pass 1 hit nothing anywhere: every vertex
/// pushed 1 model unit along its skinned normal, a halo of about a yard, won by last frame's pick,
/// else the higher priority (alive 3, dead 2), else the nearer. Every model chained to a unit is
/// its geometry ([`pick_model_roots`]); a unit with no skinned parts takes a box test at pass-1
/// level. Inert while mouse-looking or over the UI.
#[allow(clippy::type_complexity)]
pub(super) fn update_hover(
    camera: Query<(&Camera, &GlobalTransform), With<WorldCamera>>,
    window: Query<&Window, With<PrimaryWindow>>,
    rig: Res<CameraControl>,
    pointer_over_ui: Res<PointerOverUi>,
    occlusion: Res<PickOcclusion>,
    // The unit whose plate holds the pointer, as of last frame's plate layout.
    plate_hover: Res<crate::vplates::PlateHover>,
    mut hovered: ResMut<Hovered>,
    mesh_assets: Res<Assets<Mesh>>,
    pose: PickPose,
    // For `IsSelectable`'s `UNIT_FIELD_CREATEDBY` clause.
    self_guid: Res<crate::net::SelfGuid>,
    // Last frame's pick, which outranks everything in pass 2 (the reference's anti-flicker cache).
    mut last_pick: Local<Option<Entity>>,
    // Unit roots. An undrawn body is not in the reference's draw list, so it takes no mouseover,
    // cursor or click.
    roots: Query<
        (
            Entity,
            &GlobalTransform,
            &NetEntity,
            Option<&ModelAnimations>,
            Option<&AnimDriver>,
            Option<&ObjectStore>,
            Option<&Children>,
            Option<&crate::entities::mount::MountChild>,
            Option<&InheritedVisibility>,
            Option<&HeldAttached>,
        ),
        (With<Guid>, Without<SelfPlayer>),
    >,
    // The part children of every pick model root: body, worn items and mount.
    child_sets: Query<&Children>,
    parts: Query<(&Mesh3d, &benilla_world::rig_palette::RigPart)>,
    // The box fallback: every `CreaturePickPart`, the fallback cube included.
    meshes: Query<
        (
            &ChildOf,
            &Aabb,
            &GlobalTransform,
            Option<&InheritedVisibility>,
        ),
        With<CreaturePickPart>,
    >,
    // The pick's guid and kind; the kind decides which slot of `Hovered` a hit lands in.
    units: Query<(&Guid, &NetEntity), Without<SelfPlayer>>,
) {
    hovered.target = None;
    hovered.guid = None;
    hovered.corpse = None;
    hovered.corpse_guid = None;
    hovered.distance = f32::MAX;
    hovered.refused = false;
    // A plate under the pointer is the mouseover, read before the UI gate below, which the plate
    // itself trips: the reference's plate OnEnter (`0x7cb850`) publishes `[0xb4e2c8]` ungraded,
    // with no world pick behind it. Freelook wins, as the plates hand the mouse back (`0x60f830`).
    if !rig.is_looking() {
        if let Some(entity) = plate_hover.0 {
            hovered.target = Some(entity);
            hovered.guid = units.get(entity).ok().map(|(g, _)| g.0);
            hovered.distance = 0.0; // topmost UI: it beats any world GameObject at a tie
            *last_pick = Some(entity);
            return;
        }
    }
    if rig.is_looking() || pointer_over_ui.0 {
        *last_pick = None;
        return;
    }
    let (Ok((camera, cam_tf)), Ok(window)) = (camera.single(), window.single()) else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let Ok(ray) = camera.viewport_to_world(cam_tf, cursor) else {
        return;
    };
    let (origin, dir) = (ray.origin, *ray.direction);
    let limit = occlusion.distance;

    // ── Broad phase, then pass 1 and pass 2 ──
    // A candidate: the unit, its pass-2 priority, its parts' mesh ids and its posed joint palette.
    let mut candidates: Vec<(Entity, u8, Vec<AssetId<Mesh>>, Vec<Mat4>)> = Vec::new();
    // Units with skinned parts: out of the box fallback even when the broad phase rejects them.
    let mut faithful: HashSet<Entity> = HashSet::new();
    for (entity, gt, net, anims, drv, store, children, mount_child, drawn, held) in &roots {
        // Units, players and corpses: the reference picks every CGObject in one trace and
        // switches on type at the end, below.
        if !matches!(
            net.kind,
            EntityKind::Unit | EntityKind::Player | EntityKind::Corpse
        ) {
            continue;
        }
        // Not drawn, not clickable. Checked before `faithful.insert`, so an undrawn unit is out by
        // this rule, not by its exclusion from the box fallback.
        if !drawn.is_none_or(|v| v.get()) {
            continue;
        }
        let Some(children) = children else { continue };
        let worn = held.map_or(&[][..], |h| h.spawned_slots().as_slice());
        // Membership in `faithful` is decided before the broad phase rejects; the parts are
        // collected below, for the survivors only.
        if !children.iter().any(|c| parts.contains(c)) && worn.iter().all(Option::is_none) {
            continue; // static/cube unit → the AABB fallback below
        }
        faithful.insert(entity);
        // Broad phase: the playing sequence's bounds sphere, scaled by `OBJECT_FIELD_SCALE_X`. An
        // unauthored radius passes (the reference uses the header sphere), as does a mounted unit
        // (the reference sizes it from the mount's box).
        let clip = match (anims, drv) {
            (Some(a), Some(d)) => d.active_anim().and_then(|id| a.find(id)),
            _ => None,
        };
        if let Some(c) = clip.filter(|c| c.bounds_radius > 0.0 && mount_child.is_none()) {
            let centre = gt.transform_point(c.bounds_center);
            let radius = c.bounds_radius * gt.scale().max_element();
            // Reject when the ray's closest approach exceeds the radius; a centre behind the
            // origin clamps to t = 0, so a unit we stand inside stays clickable.
            let along = (centre - origin).dot(dir).max(0.0);
            if (centre - origin - dir * along).length_squared() > radius * radius {
                continue;
            }
        }
        // The skinning matrices of one skeleton's parts, as of last frame's propagated pose.
        let palette_of =
            |sk: &[(&Mesh3d, &benilla_world::rig_palette::RigPart)]| -> Option<Vec<Mat4>> {
                sk.first().and_then(|(_, part)| {
                    let rig = pose.rigs.get(part.0).ok()?;
                    pose.palettes.world_palette(rig.slot, rig.bones() as usize)
                })
            };
        // The pass-2 ladder (`0x480c90`): alive 3, dead 2, a corpse 2 when
        // `CORPSE_DYNFLAG_LOOTABLE`, else 0 (`0x480cec`). A corpse has no UNIT block, so
        // `unit_is_dead()` would answer alive for it.
        let priority = if net.kind == EntityKind::Corpse {
            u8::from(store.is_some_and(|s| s.0.corpse_lootable())) * 2
        } else if store.is_some_and(|s| s.0.unit_is_dead()) {
            2
        } else {
            3
        };
        // One candidate per model instance, all resolving to this unit. A partless body is no
        // reason to stop: a trigger creature's whole visible self can be what it wears.
        for root in pick_model_roots(entity, worn, mount_child.map(|mc| mc.0)) {
            let Ok(kids) = child_sets.get(root) else {
                continue;
            };
            let sk: Vec<_> = kids.iter().filter_map(|c| parts.get(c).ok()).collect();
            let Some(palette) = palette_of(&sk) else {
                continue;
            };
            candidates.push((
                entity,
                priority,
                sk.iter().map(|(m, _)| m.id()).collect(),
                palette,
            ));
        }
    }

    // Pass 1: the exact posed mesh, nearest wins, unbounded as in the reference; occlusion is one
    // compare on the result, below.
    let mut best: Option<(f32, Entity)> = None;
    for (entity, _, mesh_ids, palette) in &candidates {
        for id in mesh_ids {
            let Some(t) = ray_posed_mesh(&mesh_assets, *id, palette, origin, dir, false) else {
                continue;
            };
            if best.is_none_or(|(bt, _)| t < bt) {
                best = Some((t, *entity));
            }
        }
    }

    // ── The box fallback, for skinless units only ──
    // The nearest part per parent is the union's hit; `dir` is normalized, so `t` is a world
    // distance like the hits above.
    for (child_of, aabb, gt, drawn) in &meshes {
        // Not drawn, not clickable: a part inherits its root's visibility.
        if !drawn.is_none_or(|v| v.get()) {
            continue;
        }
        let parent = child_of.parent();
        if faithful.contains(&parent) {
            continue; // posed-mesh-tested above
        }
        // The same kinds as the posed pick. A bone pile lands here: its corpse model has no
        // skeleton, so only this box test can pick it.
        let Ok((_, parent_net)) = units.get(parent) else {
            continue;
        };
        if !matches!(
            parent_net.kind,
            EntityKind::Unit | EntityKind::Player | EntityKind::Corpse
        ) {
            continue;
        }
        if let Some(t) = ray_mesh_bounds(origin, dir, aabb, gt) {
            if best.is_none_or(|(bt, _)| t < bt) {
                best = Some((t, parent));
            }
        }
    }

    // Pass 2, when nothing hit exactly: the halo, won by the priority ladder, not distance.
    if best.is_none() {
        let mut best2: Option<(f32, u32, Entity)> = None;
        for (entity, priority, mesh_ids, palette) in &candidates {
            let hit = mesh_ids
                .iter()
                .filter_map(|id| ray_posed_mesh(&mesh_assets, *id, palette, origin, dir, true))
                .min_by(f32::total_cmp);
            let Some(t) = hit else { continue };
            let prio = if *last_pick == Some(*entity) {
                u32::MAX // the sticky-hover cache outranks everything
            } else {
                *priority as u32
            };
            let wins = best2.is_none_or(|(bt, bp, _)| prio > bp || (prio == bp && t < bt));
            if wins {
                best2 = Some((t, prio, *entity));
            }
        }
        best = best2.map(|(t, _, e)| (t, e));
    }

    // Occlusion (`0x480eb4`): a strictly nearer world hit drops the pick, a tie keeps it.
    if best.is_some_and(|(t, _)| limit < t) {
        best = None;
    }
    // Switch on type at the end, as the reference does: the one pick lands in the unit slot or the
    // corpse slot, and a unit the `IsSelectable` grader refuses in neither.
    if let Some((t, entity)) = best {
        if let Ok((guid, net)) = units.get(entity) {
            if net.kind == EntityKind::Corpse {
                hovered.corpse = Some(entity);
                hovered.corpse_guid = Some(guid.0);
            } else if selectable_pick(entity, &roots, self_guid.0) {
                hovered.target = Some(entity);
                hovered.guid = Some(guid.0);
            } else {
                hovered.refused = true;
            }
        } else if selectable_pick(entity, &roots, self_guid.0) {
            hovered.target = Some(entity);
        } else {
            hovered.refused = true;
        }
        // Set on every arm, the refusal included (`Hovered::refused`).
        hovered.distance = t;
    }
    *last_pick = best.map(|(_, e)| e);
}

/// The hover grader's `IsSelectable` test: `0x4828d0` calls vtable slot 21 at `0x482982`
/// (`0x60be60`, the predicate `SetSelection` runs), and a refusal jumps to `0x482090(0,0)` and
/// `ResetCursor 0x523d30`, past the publisher `0x492890`: no tooltip, cursor, brighten or click.
///
/// It grades the winner and filters no candidate: the pick (`0x480a50`, `0x480d90`, `0x713cb0`,
/// `0x481190`, `0x480df0`) reads no unit field, so a refused unit in front of a chest still takes
/// the pick ([`Hovered::refused`]). A corpse's slot 21 is `CGCorpse_C`'s, so only the unit arms
/// run this. A plate hover skips the grader (`0x7cb869`), but the plate gate `0x60f600` already
/// refuses the same units (`0x60f622`/`0x60f628`). Graded at the publish, so [`Hovered`] is never
/// readable ungraded.
#[allow(clippy::type_complexity)] // the picker's own root query, borrowed as-is
fn selectable_pick(
    entity: Entity,
    roots: &Query<
        (
            Entity,
            &GlobalTransform,
            &NetEntity,
            Option<&ModelAnimations>,
            Option<&AnimDriver>,
            Option<&ObjectStore>,
            Option<&Children>,
            Option<&crate::entities::mount::MountChild>,
            Option<&InheritedVisibility>,
            Option<&HeldAttached>,
        ),
        (With<Guid>, Without<SelfPlayer>),
    >,
    self_guid: Option<u64>,
) -> bool {
    let store = roots.get(entity).ok().and_then(|r| r.5);
    super::relations::is_selectable(store, self_guid)
}

/// The GameObject picker's state as one [`SystemParam`] (the picker is at Bevy's 16-param limit):
/// the pick-set cache and the stream edges that rebuild it, the sticky pick and the probe's locals.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct GoPickSet<'w, 's> {
    added: Query<'w, 's, (), Added<GoPickPart>>,
    removed: RemovedComponents<'w, 's, GoPickPart>,
    cache: Local<'s, bevy::platform::collections::HashSet<Entity>>,
    /// Last frame's pick by net entity, which outranks everything in pass 2.
    last_pick: Local<'s, Option<Entity>>,
    /// Every billboard card, for [`net_entity_of`]'s first hop.
    cards: Query<'w, 's, &'static BillboardCard>,
    /// The headless probe's frame counter and say-once report ([`super::hover_probe`]), idle in
    /// a player run.
    frame: Local<'s, u64>,
    report: Local<'s, super::hover_probe::ProbeReport>,
    /// `WOW_HOVER_PROBE=lock`'s held target, where the probe rests its aim for the rest of the run.
    lock: Local<'s, super::hover_probe::LockedAim>,
}

/// The most `ChildOf` hops [`net_entity_of`] climbs: a malformed-data guard, as real joint chains
/// are single digits deep.
const NET_WALK_HOPS: usize = 64;

/// A picked GameObject part's net entity, the one carrying its `Guid`. A mesh part is its child; a
/// billboard card is a world root linked by [`BillboardCard::follows`] (a joint on a rigged host,
/// else the mirror anchor), and the climb goes on from there. The reference's narrow phase walks
/// the one model's batches, billboards included (`0x7089c0`), so a card hit is a GameObject hit.
fn net_entity_of(
    e: Entity,
    cards: &Query<&BillboardCard>,
    guids: &Query<&Guid>,
    child_of: &Query<&ChildOf>,
) -> Entity {
    let mut cur = cards
        .get(e)
        .ok()
        .and_then(BillboardCard::follows)
        .unwrap_or(e);
    for _ in 0..NET_WALK_HOPS {
        if guids.contains(cur) {
            return cur;
        }
        let Ok(parent) = child_of.get(cur) else { break };
        cur = parent.parent();
    }
    cur
}

/// What the GameObject gates read beside the object's store: the template cache (GENERIC's
/// eligibility is its `data[1]`, MEETINGSTONE's its `data[2]`), the faction catalog, and the
/// meeting-stone queue, the reference's `[0xb72038]`.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct GoGateInputs<'w> {
    pub(crate) templates: Res<'w, crate::go_templates::GameObjectTemplates>,
    pub(crate) factions: Option<Res<'w, super::ring::Factions>>,
    /// Absent in a net-less build, read as the reference's zeroed `[0xb72038]`: not queued.
    pub(crate) stone: Option<Res<'w, crate::ui_dialog_verbs::MeetingStone>>,
}

impl GoGateInputs<'_> {
    /// `[0xb72038]`, the area we are queued at, or 0.
    pub(crate) fn queued_area(&self) -> u32 {
        self.stone.as_deref().map_or(0, |s| s.area)
    }
}

pub(super) fn update_hovered_object(
    camera: Query<(&Camera, &GlobalTransform), With<WorldCamera>>,
    window: Query<&Window, With<PrimaryWindow>>,
    rig: Res<CameraControl>,
    pointer_over_ui: Res<PointerOverUi>,
    occlusion: Res<PickOcclusion>,
    // The unit pick, earlier in the chain: a unit hover keeps the GameObject halo shut.
    unit_hovered: Res<Hovered>,
    mut hovered: ResMut<HoveredObject>,
    go_parts: Query<Entity, With<GoPickPart>>,
    child_of: Query<&ChildOf>,
    guids: Query<&Guid>,
    stores: Query<&ObjectStore>,
    go_gate: GoGateInputs,
    self_q: Query<&ObjectStore, With<crate::net::SelfPlayer>>,
    parts: PickParts,
    mut cache: GoPickSet,
) {
    *cache.frame = cache.frame.wrapping_add(1);
    let added_parts = &cache.added;
    let removed_parts = &mut cache.removed;
    let pickable = &mut *cache.cache;
    let last_pick = &mut *cache.last_pick;
    let cards = &cache.cards;
    let resolve_net = |e: Entity| net_entity_of(e, cards, &guids, &child_of);
    // The pick set, rebuilt only on GameObject-part stream edges (a type id never changes after
    // create). Maintained before any early return: an `Added` edge shows only on its own frame, so
    // a rebuild behind the cursor gates would drop parts streamed in while the pointer sat on UI.
    // A part despawned between edges misses at the caster.
    //
    // Transports (TRANSPORT 11, MAP_OBJECT 14, MO_TRANSPORT 15) never join: the reference's model
    // resolver `0x5f80e0` gives them no pick geometry and their interaction predicates are
    // constant false, so a ship's hull never swallows the pick of an NPC on deck.
    if removed_parts.read().next().is_some() || !added_parts.is_empty() {
        pickable.clear();
        pickable.extend(go_parts.iter().filter(|&e| {
            // Through the hit's own resolver, so a card and its mesh siblings agree.
            !stores
                .get(resolve_net(e))
                .is_ok_and(|s| matches!(s.0.gameobject_type_id(), 11 | 14 | 15))
        }));
    }
    hovered.target = None;
    hovered.guid = None;
    hovered.distance = f32::MAX;
    // The sticky pick drops the moment the pointer is not ours.
    let yielded = rig.is_looking() || pointer_over_ui.0;
    if yielded {
        *last_pick = None;
    }
    let (Ok((camera, cam_tf)), Ok(window)) = (camera.single(), window.single()) else {
        return;
    };
    // The aim: the OS pointer, else the probe's for a cursorless window. Published before the
    // yield, as the UI mouse feed reads it as its pointer: skipping it would drop `PointerOverUi`
    // and flicker the UI the probe rests on.
    let probing = super::hover_probe::armed();
    let probe_aim = cache
        .lock
        .point(camera, cam_tf, |e| parts.get(e).ok().map(|(_, gt, ..)| *gt))
        .or_else(|| super::hover_probe::point(window, *cache.frame));
    super::hover_probe::publish(probe_aim);
    if yielded {
        return;
    }
    let Some(cursor) = window.cursor_position().or(probe_aim) else {
        return;
    };
    if pickable.is_empty() {
        *last_pick = None;
        return;
    }
    let Ok(ray) = camera.viewport_to_world(cam_tf, cursor) else {
        return;
    };
    let self_store = self_q.single().ok();

    // The probe's census, once a second: where the pickable GameObjects are on screen, so a
    // headless miss tells a pick fault from a camera looking elsewhere.
    if probing && (*cache.frame).is_multiple_of(60) {
        let rows: Vec<String> = pickable
            .iter()
            .filter_map(|&e| {
                let (_, gt, vis, ..) = parts.get(e).ok()?;
                let world = gt.translation();
                let screen = camera.world_to_viewport(cam_tf, world).ok();
                Some(format!(
                    "{:?}{} → {}",
                    resolve_net(e),
                    if vis.get() { "" } else { " (unseen)" },
                    screen.map_or_else(
                        || "off-camera".to_string(),
                        |p| format!("{:.0},{:.0}", p.x, p.y)
                    ),
                ))
            })
            .take(8)
            .collect();
        info!(
            "hover probe census: window {:.0}x{:.0}, {} pickable — {}",
            window.width(),
            window.height(),
            pickable.len(),
            rows.join(" · ")
        );
    }

    // Pass 1: the exact resident geometry, nearest wins.
    let mut best: Option<(f32, Entity)> =
        benilla_world::interact::cast_pick_ray(ray, pickable, &parts, false)
            .into_iter()
            .next()
            .map(|(e, h)| (h.distance, e));

    // Pass 2, only when nothing hit anywhere, units included (one resolve in the reference): the
    // geometry pushed 1 model unit along its normals, a halo of about a yard that makes a wispy
    // herb clickable around its leaves. Every unit outranks every GameObject on that ladder, so a
    // unit hover keeps it shut. The reference's sticky pick is cross-type and can hold a
    // GameObject over a unit's halo; here the unit wins that frame.
    if best.is_none() {
        if unit_hovered.target.is_some() {
            *last_pick = None;
            return;
        }
        let mut best2: Option<(f32, u32, Entity)> = None;
        for (part, hit) in benilla_world::interact::cast_pick_ray_inflated(ray, pickable, &parts) {
            let net = resolve_net(part);
            let prio = if *last_pick == Some(net) {
                u32::MAX // the sticky-hover cache outranks everything
            } else {
                // `0x480c90`: highlightable 1, else 0. A store not yet streamed reads
                // highlightable, the eligibility gate's own default.
                stores.get(net).map_or(1, |s| {
                    let reaction = crate::target::cursor_mode::go_reaction(
                        go_gate.factions.as_deref(),
                        s.0.gameobject_faction(),
                        self_store,
                    );
                    let go_guid = guids.get(net).ok().map(|g| g.0);
                    let overrides = crate::target::cursor_mode::GoOverrides {
                        channel_owned: crate::target::cursor_mode::fishing_channel_owned(
                            self_store, go_guid,
                        ),
                        meeting_stone_queued: crate::target::cursor_mode::meeting_stone_queued(
                            go_guid
                                .and_then(|g| go_gate.templates.get(g)?.meeting_stone)
                                .map(|m| m.area),
                            go_gate.queued_area(),
                        ),
                    };
                    u32::from(crate::target::cursor_mode::go_highlightable(
                        s, reaction, overrides,
                    ))
                })
            };
            let wins =
                best2.is_none_or(|(bt, bp, _)| prio > bp || (prio == bp && hit.distance < bt));
            if wins {
                best2 = Some((hit.distance, prio, part));
            }
        }
        best = best2.map(|(t, _, e)| (t, e));
    }

    // Occlusion (`0x480eb4`): a strictly nearer world hit drops the pick, a tie keeps it.
    if best.is_some_and(|(t, _)| occlusion.distance < t) {
        best = None;
    }
    let picked = best.map(|(t, e)| (t, resolve_net(e)));
    *last_pick = picked.map(|(_, net)| net);
    let Some((distance, net_entity)) = picked else {
        if probing {
            cache.report.say(format!(
                "aim {cursor:?} — no GameObject hit (pickable {})",
                pickable.len()
            ));
        }
        return;
    };
    let Ok(guid) = guids.get(net_entity) else {
        return;
    };
    // The mouseover-eligibility gate (vtable `+0x54` at `0x482982`): an ineligible object
    // publishes the null mouseover, so no tooltip, brighten or cursor; the reference reaches
    // `0x492890`, `0x52aa20` and `0x4945e0` only past it. Its null mouseover also hides a unit
    // behind the object, which our split picks leave hoverable.
    if let Ok(store) = stores.get(net_entity) {
        let tmpl = go_gate.templates.get(guid.0);
        let reaction = crate::target::cursor_mode::go_reaction(
            go_gate.factions.as_deref(),
            store.0.gameobject_faction(),
            self_store,
        );
        if !crate::target::cursor_mode::mouseover_eligible(
            store.0.gameobject_type_id(),
            store.0.gameobject_flags(),
            store.0.gameobject_dynamic_flags(),
            tmpl.map(|t| t.highlight_column),
            reaction,
            crate::target::cursor_mode::GoOverrides {
                channel_owned: crate::target::cursor_mode::fishing_channel_owned(
                    self_store,
                    Some(guid.0),
                ),
                meeting_stone_queued: crate::target::cursor_mode::meeting_stone_queued(
                    tmpl.and_then(|t| t.meeting_stone).map(|m| m.area),
                    go_gate.queued_area(),
                ),
            },
        ) {
            if probing {
                cache.report.say(format!(
                    "guid {:#x} type {} at {distance:.1} yd — hover ✗ (tmpl {:?}, highlight {:?}): \
                     the eligibility gate refused it, so there is no tooltip BY DESIGN",
                    guid.0,
                    store.0.gameobject_type_id(),
                    tmpl.map(|t| t.name.as_str()),
                    tmpl.map(|t| t.highlight_column),
                ));
            }
            return;
        }
        if probing {
            cache.report.say(format!(
                "guid {:#x} type {} at {distance:.1} yd — hover ✓, published (tmpl {:?}, highlight \
                 {:?}, occlusion {:.1})",
                guid.0,
                store.0.gameobject_type_id(),
                tmpl.map(|t| t.name.as_str()),
                tmpl.map(|t| t.highlight_column),
                occlusion.distance,
            ));
            // `lock` holds only an object past the eligibility gate, the hit kept in the part's
            // frame so the aim survives the camera and the object moving.
            if super::hover_probe::locks() {
                if let Some((t, part)) = best {
                    if let Ok((_, gt, ..)) = parts.get(part) {
                        cache.lock.hold(part, gt, ray.origin + *ray.direction * t);
                    }
                }
            }
        }
    }
    hovered.target = Some(net_entity);
    hovered.guid = Some(guid.0);
    hovered.distance = distance;
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    fn card(bone: u16, owner: Entity) -> BillboardCard {
        let info = benilla_assets::BillboardInfo {
            pivot: Vec3::ZERO,
            bone,
            kind: benilla_formats::BillboardKind::LockZ,
            scale_anim: None,
            seq_translations: vec![],
        };
        BillboardCard::following(&info, owner)
    }

    /// Both card shapes `spawn_billboard_part` makes: following the mirror anchor (a rigless host)
    /// and following a nested joint (a rigged one).
    #[test]
    fn a_billboard_cards_hit_resolves_to_its_gameobject() {
        let mut world = World::new();
        let net = world.spawn(Guid(0xdead_beef)).id();
        // An ordinary mesh part, a direct child.
        let mesh_part = world.spawn(ChildOf(net)).id();
        // The rigless shape: the card follows the mirror anchor under the net entity.
        let anchor = world.spawn(ChildOf(net)).id();
        let anchor_card = world.spawn(card(0, anchor)).id();
        // The rigged shape: net → model root → root-bone joint → child joint, card on the deepest.
        let root = world.spawn(ChildOf(net)).id();
        let joint0 = world.spawn(ChildOf(root)).id();
        let joint1 = world.spawn(ChildOf(joint0)).id();
        let joint_card = world.spawn(card(1, joint1)).id();
        // A card whose owner is gone (despawned mid-frame) must not climb into someone else's tree.
        let orphan_card = world
            .spawn(card(2, Entity::from_raw_u32(9999).unwrap()))
            .id();

        let resolved = world
            .run_system_once(
                move |cards: Query<&BillboardCard>,
                      guids: Query<&Guid>,
                      child_of: Query<&ChildOf>|
                      -> Vec<Entity> {
                    [mesh_part, anchor_card, joint_card, orphan_card, net]
                        .into_iter()
                        .map(|e| net_entity_of(e, &cards, &guids, &child_of))
                        .collect()
                },
            )
            .unwrap();

        assert_eq!(
            resolved[0], net,
            "mesh part: the one-hop case still resolves"
        );
        assert_eq!(
            resolved[1], net,
            "anchor-following card: the world root hops through `follows()`, not `ChildOf`"
        );
        assert_eq!(
            resolved[2], net,
            "joint-following card: the climb continues up the joint hierarchy, not one hop"
        );
        assert_ne!(
            resolved[3], net,
            "a card whose owner is gone resolves to nothing that carries a guid"
        );
        assert_eq!(resolved[4], net, "the net entity resolves to itself");
    }

    /// Naxxramas's "Unholy Axe" is an `InvisibleStalker` body with no vertices holding a real axe,
    /// which is its whole pick surface.
    #[test]
    fn a_units_pick_geometry_is_every_model_chained_to_it() {
        let unit = Entity::from_raw_u32(1).unwrap();
        let main_hand = Entity::from_raw_u32(2).unwrap();
        let helm = Entity::from_raw_u32(3).unwrap();
        let mount = Entity::from_raw_u32(4).unwrap();

        let bare: Vec<_> = pick_model_roots(unit, &[], None).collect();
        assert_eq!(bare, vec![unit], "a unit wearing nothing is just its body");

        // Empty slots, as in the real `HeldAttached` (main, off, ranged, helm, both shoulders,
        // ammo, quiver), are not candidates.
        let worn = [
            Some(main_hand),
            None,
            None,
            Some(helm),
            None,
            None,
            None,
            None,
        ];
        let dressed: Vec<_> = pick_model_roots(unit, &worn, Some(mount)).collect();
        assert_eq!(
            dressed,
            vec![unit, main_hand, helm, mount],
            "body, then every worn model, then the mount — all one unit's geometry"
        );

        // A body that draws nothing: the axe is the whole hit surface.
        let trigger: Vec<_> = pick_model_roots(unit, &worn[..1], None).collect();
        assert!(
            trigger.contains(&main_hand),
            "a body with no geometry of its own still offers what it holds"
        );
    }
}
