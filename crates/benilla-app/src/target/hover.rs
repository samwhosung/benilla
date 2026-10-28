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
    ray_mesh_bounds, ray_posed_mesh, CreaturePickPart, DoodadOccluders, GoPickPart, PickParts,
};
use benilla_world::view::WorldCamera;

use super::{Hovered, HoveredObject, PickOcclusion};

/// What the world trace casts against, as one [`SystemParam`]: the reference's `CWorld::Intersect`
/// leg of the scene trace (`0x480df0`, mask `0x1000114`): the [`PickOccluder`] colliders, terrain
/// and the WMO walk-bake faces (about the `0x84` reject mask), and the static doodads' drawn
/// opaque batches ([`DoodadOccluders`]), never their collision hulls. Net entities are the object
/// trace, not occluders, so a chest never occludes itself. The picks run unbounded and compare
/// against it after.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct OcclusionTrace<'w, 's> {
    spatial: avian3d::prelude::SpatialQuery<'w, 's>,
    occluders: Query<'w, 's, (), With<PickOccluder>>,
    doodads: DoodadOccluders<'w, 's>,
}

impl OcclusionTrace<'_, '_> {
    /// The world hit along `ray`.
    pub(super) fn trace(&self, ray: Ray3d) -> PickOcclusion {
        let mut occlusion = PickOcclusion {
            ray: Some(ray.direction),
            ..PickOcclusion::default()
        };
        let collider = self
            .spatial
            .cast_ray_predicate(
                ray.origin,
                ray.direction,
                f32::MAX,
                true,
                // The mask that reaches every `PickOccluder` collider; the marker picks the set
                // within.
                &benilla_world::collision::WorldCollision::body_filter(),
                &|e| self.occluders.contains(e),
            )
            .map(|hit| hit.distance);
        let nearest = match (collider, self.doodads.nearest(ray)) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        if let Some(distance) = nearest {
            occlusion.distance = distance;
            occlusion.point = Some(ray.origin + *ray.direction * distance);
        }
        occlusion
    }
}

/// The ray through `cursor` from the world camera, `None` with no camera or one that cannot
/// project.
fn cursor_ray(
    camera: &Query<(&Camera, &GlobalTransform), With<WorldCamera>>,
    cursor: Vec2,
) -> Option<Ray3d> {
    let (camera, cam_tf) = camera.single().ok()?;
    camera.viewport_to_world(cam_tf, cursor).ok()
}

/// Trace this frame's world hit along the cursor ray ([`OcclusionTrace`]).
pub(super) fn update_pick_occlusion(
    camera: Query<(&Camera, &GlobalTransform), With<WorldCamera>>,
    window: Query<&Window, With<PrimaryWindow>>,
    trace: OcclusionTrace,
    probe: Res<super::hover_probe::ProbeAim>,
    mut occlusion: ResMut<PickOcclusion>,
) {
    *occlusion = PickOcclusion::default();
    let Ok(window) = window.single() else {
        return;
    };
    // A cursorless window traces from the hover probe's aim, as the picks do.
    let Some(cursor) = window.cursor_position().or_else(|| probe.now()) else {
        return;
    };
    let Some(ray) = cursor_ray(&camera, cursor) else {
        return;
    };
    *occlusion = trace.trace(ray);
}

/// The hover probe's aim for the frame, then the cursor ray's world trace from it, so the trace
/// and the object pick after it read the same point.
pub(super) fn aim_and_trace(
) -> bevy::ecs::schedule::ScheduleConfigs<bevy::ecs::system::ScheduleSystem> {
    (super::hover_probe::aim_frame, update_pick_occlusion).chain()
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

/// Every model's current skeleton pose, as one [`SystemParam`]: the palette rows and the per-part
/// rig link that indexes them.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct PickPose<'w, 's> {
    /// The world-space matrices the vertex stage skins with, read from the CPU rows.
    palettes: Res<'w, benilla_world::rig_palette::RigPalettes>,
    rigs: Query<'w, 's, &'static benilla_world::rig_palette::RigSkin>,
}

/// The local identity and what of it and of the corpses the pick takes, as one [`SystemParam`].
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct TargetHoverState<'w, 's> {
    /// For `IsSelectable`'s `UNIT_FIELD_CREATEDBY` clause and the pick filter's possession clause.
    self_guid: Res<'w, crate::net::SelfGuid>,
    /// `0x6e61a0`'s pick flag `0x20`, as the feed published it.
    picks_self: Res<'w, crate::spell::PicksSelf>,
    /// The corpse leg's word, as the feed published it.
    corpse_pick: Res<'w, crate::spell::CorpsePick>,
    /// The corpse reaction gate's inputs (`0x6067d0`).
    factions: Option<Res<'w, super::Factions>>,
    self_store: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    /// The unit and player filter while a word stands (`0x480610`).
    units: crate::spell::targeting::PickChecks<'w, 's>,
}

impl TargetHoverState<'_, '_> {
    /// `0x480610`'s two exclusions, which come first and which a pick whose flags carry `0x20`
    /// skips (`48062c`, [`crate::spell::PicksSelf`]; `0x481050` ORs it in only for a word
    /// `0x6e61a0` passes, so outside targeting they always run): the active player (`480638`),
    /// and a unit they possess, an Eye of Kilrogg or a Mind Controlled target
    /// ([`super::is_possessed_by`], `48064f`-`480674`), so the cursor and the click pass through
    /// it to what lies behind.
    fn excludes(&self, is_self: bool, store: Option<&ObjectStore>) -> bool {
        !self.picks_self.0 && (is_self || super::is_possessed_by(store, self.self_guid.0))
    }

    /// Whether an object of this kind is a candidate of this pick, the posed pass and the box
    /// fallback alike: every unit and player outside targeting but the two [`Self::excludes`]
    /// drops, and while targeting the ones `0x480610` passes for the word
    /// ([`crate::spell::targeting::PickChecks::admits_unit`]); a corpse (`0x480816`) always
    /// outside targeting, whose pick flags carry `0x40`, and while targeting only one `0x6e6260`
    /// passes. A corpse whose store has not streamed is refused then. `is_self` is the active
    /// player's own body. The hover and the press share it.
    fn is_candidate(
        &self,
        entity: Entity,
        kind: EntityKind,
        is_self: bool,
        store: Option<&ObjectStore>,
    ) -> bool {
        match kind {
            EntityKind::Unit | EntityKind::Player => {
                !self.excludes(is_self, store) && self.units.admits_unit(entity, store)
            }
            EntityKind::Corpse => {
                let Some(word) = self.corpse_pick.0 else {
                    return true;
                };
                store.is_some_and(|store| {
                    crate::spell::targeting::corpse_pick_admits(
                        word,
                        store,
                        self.factions.as_deref(),
                        self.self_store.iter().next(),
                    )
                })
            }
            _ => false,
        }
    }
}

/// The pick's sticky caches, the hover's and the press's alike: the one `PickAtCursor` (`0x481190`)
/// keeps, which both the mouseover (`0x48184a`) and the press (`0x481f47`) call, so the last pick
/// at either outranks the rest of the halo on the next.
#[derive(Resource, Default)]
pub(crate) struct StickyPicks {
    /// The last unit, player or corpse picked.
    pub(super) unit: Option<Entity>,
    /// The last GameObject picked, by net entity.
    pub(super) object: Option<Entity>,
}

/// The posed pick's geometry under one model root: its skinned parts. Ours are dropped while
/// hidden: first person hides them ([`crate::player::apply_self_model_fade`] at alpha 0), where the
/// reference's camera fade clears its player-visible flag `[0xc4d74c]` (`0x482c41`) and
/// `ShouldRender 0x5ec8f0` refuses the active player, so it is never picked.
#[allow(clippy::type_complexity)] // the picker's own part query, borrowed as-is
fn posed_parts<'a>(
    kids: &Children,
    parts: &'a Query<(
        &Mesh3d,
        &benilla_world::rig_palette::RigPart,
        Option<&InheritedVisibility>,
    )>,
    is_self: bool,
) -> Vec<(&'a Mesh3d, &'a benilla_world::rig_palette::RigPart)> {
    kids.iter()
        .filter_map(|c| parts.get(c).ok())
        .filter(|(_, _, drawn)| !is_self || drawn.is_none_or(|v| v.get()))
        .map(|(mesh, part, _)| (mesh, part))
        .collect()
}

/// The unit roots the pick walks. An undrawn body is not in the reference's draw list, so it takes
/// no mouseover, cursor or click.
type PickRoots<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        Has<SelfPlayer>,
        &'static GlobalTransform,
        &'static NetEntity,
        Option<&'static ModelAnimations>,
        Option<&'static AnimDriver>,
        Option<&'static ObjectStore>,
        Option<&'static Children>,
        Option<&'static crate::entities::mount::MountChild>,
        Option<&'static InheritedVisibility>,
        Option<&'static HeldAttached>,
    ),
    With<Guid>,
>;

/// The unit pick's inputs, as one [`SystemParam`] the hover and the press share.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct UnitPicker<'w, 's> {
    mesh_assets: Res<'w, Assets<Mesh>>,
    pose: PickPose<'w, 's>,
    // Our own body joins the pick only while a targeting word takes it (`0x6e61a0`).
    target_state: TargetHoverState<'w, 's>,
    roots: PickRoots<'w, 's>,
    // The part children of every pick model root: body, worn items and mount.
    child_sets: Query<'w, 's, &'static Children>,
    #[allow(clippy::type_complexity)]
    parts: Query<
        'w,
        's,
        (
            &'static Mesh3d,
            &'static benilla_world::rig_palette::RigPart,
            Option<&'static InheritedVisibility>,
        ),
    >,
    // The box fallback: every `CreaturePickPart`, the fallback cube included.
    #[allow(clippy::type_complexity)]
    meshes: Query<
        'w,
        's,
        (
            &'static ChildOf,
            &'static Aabb,
            &'static GlobalTransform,
            Option<&'static InheritedVisibility>,
        ),
        With<CreaturePickPart>,
    >,
    // The pick's guid and kind; the kind decides which slot of `Hovered` a hit lands in.
    units: Query<'w, 's, (&'static Guid, &'static NetEntity)>,
}

/// A pick candidate: the unit, its pass-2 priority, its parts' mesh ids and its posed palette.
type Candidate = (Entity, u8, Vec<AssetId<Mesh>>, Vec<Mat4>);

impl UnitPicker<'_, '_> {
    /// A plate's unit as the pick: the plate's enter publishes it ungraded (`0x7cb850`), with no
    /// world pick behind it, and it beats any world GameObject at a tie.
    pub(super) fn plate(&self, entity: Entity, sticky: &mut Option<Entity>) -> Hovered {
        *sticky = Some(entity);
        Hovered {
            target: Some(entity),
            guid: self.units.get(entity).ok().map(|(g, _)| g.0),
            distance: 0.0,
            plate: true,
            ..Hovered::default()
        }
    }

    /// The unit under `ray`, as the reference's pick (`0x7089c0`) finds it. Broad phase: the ray
    /// against the playing sequence's bounds sphere. Pass 1: the ray against the posed render mesh,
    /// nearest wins. Pass 2, the mouse pick's retry when pass 1 hit nothing anywhere: every vertex
    /// pushed 1 model unit along its skinned normal, a halo of about a yard, won by the last pick,
    /// else the higher priority (alive 3, dead 2), else the nearer. Every model chained to a unit
    /// is its geometry ([`pick_model_roots`]); a unit with no skinned parts takes a box test at
    /// pass-1 level. A world hit nearer than `limit` drops the pick. `sticky` is the last pick, which
    /// outranks everything in pass 2 (the reference's anti-flicker cache).
    pub(super) fn pick(&self, ray: Ray3d, limit: f32, sticky: &mut Option<Entity>) -> Hovered {
        let mut hovered = Hovered {
            distance: f32::MAX,
            ..Hovered::default()
        };
        let (origin, dir) = (ray.origin, *ray.direction);
        let Self {
            mesh_assets,
            pose,
            target_state,
            roots,
            child_sets,
            parts,
            meshes,
            units,
        } = self;

        // ── Broad phase, then pass 1 and pass 2 ──
        let mut candidates: Vec<Candidate> = Vec::new();
        // Units with skinned parts: out of the box fallback even when the broad phase rejects them.
        let mut faithful: HashSet<Entity> = HashSet::new();
        for (entity, is_self, gt, net, anims, drv, store, children, mount_child, drawn, held) in
            roots.iter()
        {
            // Units, players and corpses: the reference picks every CGObject in one trace and
            // switches on type at the end, below.
            if !target_state.is_candidate(entity, net.kind, is_self, store) {
                continue;
            }
            // Not drawn, not clickable. Checked before `faithful.insert`, so an undrawn unit is out
            // by this rule, not by its exclusion from the box fallback.
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
            // Broad phase: the playing sequence's bounds sphere, scaled by `OBJECT_FIELD_SCALE_X`.
            // An unauthored radius passes (the reference uses the header sphere), as does a mounted
            // unit (the reference sizes it from the mount's box).
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
            // `CORPSE_DYNFLAG_LOOTABLE`, else 0 (`0x480cec`). A corpse has no UNIT block, so its
            // health reads 0 and `unit_is_dead()` would answer dead for it.
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
                let sk = posed_parts(kids, parts, is_self);
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

        // Pass 1: the exact posed mesh, nearest wins, unbounded as in the reference; occlusion is
        // one compare on the result, below.
        let mut best: Option<(f32, Entity)> = None;
        for (entity, _, mesh_ids, palette) in &candidates {
            for id in mesh_ids {
                let Some(t) = ray_posed_mesh(mesh_assets, *id, palette, origin, dir, false) else {
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
        for (child_of, aabb, gt, drawn) in meshes.iter() {
            // Not drawn, not clickable: a part inherits its root's visibility.
            if !drawn.is_none_or(|v| v.get()) {
                continue;
            }
            let parent = child_of.parent();
            if faithful.contains(&parent) {
                continue; // posed-mesh-tested above
            }
            // The same candidates as the posed pick. A bone pile lands here: its corpse model has
            // no skeleton, so only this box test can pick it.
            let Ok((_, parent_net)) = units.get(parent) else {
                continue;
            };
            let root = roots.get(parent).ok();
            let is_self = root.as_ref().is_some_and(|root| root.1);
            let store = root.and_then(|root| root.6);
            if !target_state.is_candidate(parent, parent_net.kind, is_self, store) {
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
                    .filter_map(|id| ray_posed_mesh(mesh_assets, *id, palette, origin, dir, true))
                    .min_by(f32::total_cmp);
                let Some(t) = hit else { continue };
                let prio = if *sticky == Some(*entity) {
                    u32::MAX // the sticky cache outranks everything
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
        // Switch on type at the end, as the reference does: the one pick lands in the unit slot or
        // the corpse slot, and a unit the `IsSelectable` grader refuses in neither. While a word
        // stands the grader forks to its targeting arm (`0x4828f4`) and publishes the pick ungraded
        // (`0x4829d4`-`0x4829e0`), and the click binds it ahead of its own test
        // (`0x4935ca`-`0x4935d5`).
        let graded = |entity| {
            target_state.units.targeting()
                || selectable_pick(entity, roots, target_state.self_guid.0)
        };
        if let Some((t, entity)) = best {
            if let Ok((guid, net)) = units.get(entity) {
                if net.kind == EntityKind::Corpse {
                    hovered.corpse = Some(entity);
                    hovered.corpse_guid = Some(guid.0);
                } else if graded(entity) {
                    hovered.target = Some(entity);
                    hovered.guid = Some(guid.0);
                } else {
                    hovered.refused = true;
                }
            } else if graded(entity) {
                hovered.target = Some(entity);
            } else {
                hovered.refused = true;
            }
            // Set on every arm, the refusal included (`Hovered::refused`).
            hovered.distance = t;
        }
        *sticky = best.map(|(_, e)| e);
        hovered
    }
}

/// The unit under the cursor this frame ([`UnitPicker::pick`]), or the hovered plate's. Inert while
/// mouse-looking or over the UI.
pub(super) fn update_hover(
    camera: Query<(&Camera, &GlobalTransform), With<WorldCamera>>,
    window: Query<&Window, With<PrimaryWindow>>,
    rig: Res<CameraControl>,
    pointer_over_ui: Res<PointerOverUi>,
    occlusion: Res<PickOcclusion>,
    // The unit whose plate holds the hover after this frame's UI walk.
    plate_hover: Res<crate::vplates::PlateHover>,
    mut hovered: ResMut<Hovered>,
    picker: UnitPicker,
    mut sticky: ResMut<StickyPicks>,
) {
    *hovered = Hovered {
        distance: f32::MAX,
        ..Hovered::default()
    };
    // A plate under the pointer is the mouseover, read before the UI gate below, which the plate
    // itself trips: the reference's plate OnEnter (`0x7cb850`) publishes `[0xb4e2c8]` ungraded,
    // with no world pick behind it. Freelook wins, as the plates hand the mouse back (`0x60f830`).
    if !rig.is_looking() {
        if let Some(entity) = plate_hover.0 {
            *hovered = picker.plate(entity, &mut sticky.unit);
            return;
        }
    }
    if rig.is_looking() || pointer_over_ui.0 {
        sticky.unit = None;
        return;
    }
    let Some(cursor) = window.single().ok().and_then(Window::cursor_position) else {
        return;
    };
    let Some(ray) = cursor_ray(&camera, cursor) else {
        return;
    };
    *hovered = picker.pick(ray, occlusion.distance, &mut sticky.unit);
}

/// The hover grader's `IsSelectable` test, run outside targeting only: `0x4828d0` calls vtable
/// slot 21 at `0x482982` (`0x60be60`, the predicate `SetSelection` runs), and a refusal jumps to
/// `0x482090(0,0)` and `ResetCursor 0x523d30`, past the publisher `0x492890`: no tooltip, cursor,
/// brighten or click.
///
/// It grades the winner and filters no candidate: the pick (`0x480a50`, `0x480d90`, `0x713cb0`,
/// `0x481190`, `0x480df0`) reads no unit field, so a refused unit in front of a chest still takes
/// the pick ([`Hovered::refused`]). A corpse's slot 21 is `CGCorpse_C`'s, so only the unit arms
/// run this. A plate hover skips the grader (`0x7cb869`), but the plate gate `0x60f600` already
/// refuses the same units (`0x60f622`/`0x60f628`). Graded at the publish, so [`Hovered`] is never
/// readable ungraded.
fn selectable_pick(entity: Entity, roots: &PickRoots, self_guid: Option<u64>) -> bool {
    let store = roots.get(entity).ok().and_then(|r| r.6);
    super::relations::is_selectable(store, self_guid)
}

/// The GameObject parts the world pick may hit, which [`track_go_pick_parts`] keeps and every
/// picker reads: the hover, a press's pick and an arm's.
#[derive(Resource, Default)]
pub(crate) struct GoPickable(bevy::platform::collections::HashSet<Entity>);

/// Keep [`GoPickable`], rebuilt only on GameObject-part stream edges (a type id never changes after
/// create), once a frame ahead of the frame's input records, so every pick that frame reads it.
/// A part despawned between edges misses at the caster.
///
/// Transports (TRANSPORT 11, MAP_OBJECT 14, MO_TRANSPORT 15) never join: the reference's model
/// resolver `0x5f80e0` gives them no pick geometry and their interaction predicates are constant
/// false, so a ship's hull never swallows the pick of an NPC on deck.
#[allow(clippy::too_many_arguments)]
pub(super) fn track_go_pick_parts(
    added: Query<(), Added<GoPickPart>>,
    mut removed: RemovedComponents<GoPickPart>,
    go_parts: Query<Entity, With<GoPickPart>>,
    cards: Query<&BillboardCard>,
    guids: Query<&Guid>,
    child_of: Query<&ChildOf>,
    stores: Query<&ObjectStore>,
    mut pickable: ResMut<GoPickable>,
) {
    if removed.read().next().is_none() && added.is_empty() {
        return;
    }
    let resolve_net = |e: Entity| net_entity_of(e, &cards, &guids, &child_of);
    pickable.0.clear();
    pickable.0.extend(go_parts.iter().filter(|&e| {
        // Through the hit's own resolver, so a card and its mesh siblings agree.
        !stores
            .get(resolve_net(e))
            .is_ok_and(|s| matches!(s.0.gameobject_type_id(), 11 | 14 | 15))
    }));
}

/// The GameObject picker's state as one [`SystemParam`]: the pick set, and the probe's locals.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct GoPickSet<'w, 's> {
    pickable: Res<'w, GoPickable>,
    /// Every billboard card, for [`net_entity_of`]'s first hop.
    cards: Query<'w, 's, &'static BillboardCard>,
    /// The headless probe's frame ([`super::hover_probe`]) and say-once report, idle in a player
    /// run.
    probe: ResMut<'w, super::hover_probe::ProbeAim>,
    report: Local<'s, super::hover_probe::ProbeReport>,
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

/// The GameObject pick's inputs, as one [`SystemParam`] the hover and the press share.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct ObjectPicker<'w, 's> {
    child_of: Query<'w, 's, &'static ChildOf>,
    guids: Query<'w, 's, &'static Guid>,
    stores: Query<'w, 's, &'static ObjectStore>,
    go_gate: GoGateInputs<'w>,
    self_q: Query<'w, 's, &'static ObjectStore, With<crate::net::SelfPlayer>>,
    parts: PickParts<'w, 's>,
    cache: GoPickSet<'w, 's>,
    // The GameObject arm's flag `0x4` and `0x6e62e0` while a word stands (`0x480780`).
    word: crate::spell::targeting::GoPickChecks<'w, 's>,
}

impl ObjectPicker<'_, '_> {
    /// The probe's aim and whether it is armed, for a cursorless window.
    pub(super) fn probe_aim(&self) -> (bool, Option<Vec2>) {
        (self.cache.probe.armed(), self.cache.probe.now())
    }

    /// The GameObject under `ray`, cast through `cursor` (for the probe's report), against the
    /// world hit `occlusion` and the unit pick `unit_hovered`, whose hover keeps the halo shut;
    /// `sticky` is the last GameObject picked, which outranks everything in pass 2.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn pick(
        &mut self,
        ray: Ray3d,
        camera: (&Camera, &GlobalTransform),
        window: &Window,
        cursor: Vec2,
        occlusion: &PickOcclusion,
        unit_hovered: &Hovered,
        sticky: &mut Option<Entity>,
    ) -> HoveredObject {
        let nothing = HoveredObject {
            target: None,
            guid: None,
            distance: f32::MAX,
        };
        let Self {
            child_of,
            guids,
            stores,
            go_gate,
            self_q,
            parts,
            cache,
            word,
            ..
        } = self;
        let probing = cache.probe.armed();
        let pickable = &cache.pickable.0;
        let cards = &cache.cards;
        let resolve_net = |e: Entity| net_entity_of(e, cards, guids, child_of);
        if pickable.is_empty() {
            *sticky = None;
            return nothing;
        }
        let self_store = self_q.single().ok();
        // While a word stands, only the objects `0x480780`'s GameObject arm admits are candidates:
        // none for a word outside `0x4800`, else those whose lock the spell passes.
        let admitted: bevy::platform::collections::HashSet<Entity>;
        let pickable: &bevy::platform::collections::HashSet<Entity> = if word.targeting() {
            admitted = pickable
                .iter()
                .copied()
                .filter(|&part| word.admits(resolve_net(part)))
                .collect();
            &admitted
        } else {
            pickable
        };
        if pickable.is_empty() {
            *sticky = None;
            return nothing;
        }

        // The probe's census, once a second: where the pickable GameObjects are on screen, so a
        // headless miss tells a pick fault from a camera looking elsewhere.
        if probing && cache.probe.frame().is_multiple_of(60) {
            let (camera, cam_tf) = camera;
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
            benilla_world::interact::cast_pick_ray(ray, pickable, parts, false)
                .into_iter()
                .next()
                .map(|(e, h)| (h.distance, e));

        // Pass 2, only when nothing hit anywhere, units included (one resolve in the reference):
        // the geometry pushed 1 model unit along its normals, a halo of about a yard that makes a
        // wispy herb clickable around its leaves. Every unit outranks every GameObject on that
        // ladder, so a unit hover keeps it shut. The reference's sticky pick is cross-type and can
        // hold a GameObject over a unit's halo; here the unit wins that pick.
        if best.is_none() {
            if unit_hovered.target.is_some() {
                *sticky = None;
                return nothing;
            }
            let mut best2: Option<(f32, u32, Entity)> = None;
            for (part, hit) in benilla_world::interact::cast_pick_ray_inflated(ray, pickable, parts)
            {
                let net = resolve_net(part);
                let prio = if *sticky == Some(net) {
                    u32::MAX // the sticky cache outranks everything
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
                                go_gate
                                    .templates
                                    .of(&s.0)
                                    .and_then(|t| t.meeting_stone)
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
        *sticky = picked.map(|(_, net)| net);
        let Some((distance, net_entity)) = picked else {
            if probing {
                cache.report.say(format!(
                    "aim {cursor:?} — no GameObject hit (pickable {})",
                    pickable.len()
                ));
            }
            return nothing;
        };
        let Ok(guid) = guids.get(net_entity) else {
            return nothing;
        };
        // The mouseover-eligibility gate (vtable `+0x54` at `0x482982`): an ineligible object
        // publishes the null mouseover, so no tooltip, brighten or cursor; the reference reaches
        // `0x492890`, `0x52aa20` and `0x4945e0` only past it. Its null mouseover also hides a unit
        // behind the object, which our split picks leave hoverable. While a word stands the grader
        // forks before it (`0x4828f4`) and publishes what the pick admitted (`0x4829d4`).
        if let Some(store) = stores.get(net_entity).ok().filter(|_| !word.targeting()) {
            let tmpl = go_gate.templates.of(&store.0);
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
                        "guid {:#x} type {} at {distance:.1} yd — hover ✗ (tmpl {:?}, highlight \
                         {:?}): the eligibility gate refused it, so there is no tooltip BY DESIGN",
                        guid.0,
                        store.0.gameobject_type_id(),
                        tmpl.map(|t| t.name.as_str()),
                        tmpl.map(|t| t.highlight_column),
                    ));
                }
                return nothing;
            }
            if probing {
                cache.report.say(format!(
                    "guid {:#x} type {} at {distance:.1} yd — hover ✓, published (tmpl {:?}, \
                     highlight {:?}, occlusion {:.1})",
                    guid.0,
                    store.0.gameobject_type_id(),
                    tmpl.map(|t| t.name.as_str()),
                    tmpl.map(|t| t.highlight_column),
                    occlusion.distance,
                ));
                // `lock` holds only an object past the eligibility gate, the hit kept in the
                // part's frame so the aim survives the camera and the object moving.
                if cache.probe.locks() {
                    if let Some((t, part)) = best {
                        if let Ok((_, gt, ..)) = parts.get(part) {
                            cache
                                .probe
                                .lock
                                .hold(part, gt, ray.origin + *ray.direction * t);
                        }
                    }
                }
            }
        }
        HoveredObject {
            target: Some(net_entity),
            guid: Some(guid.0),
            distance,
        }
    }
}

/// The GameObject under the cursor this frame ([`ObjectPicker::pick`]), the aim the probe's for a
/// cursorless window ([`super::hover_probe::aim_frame`]). Inert while mouse-looking or over the UI.
pub(super) fn update_hovered_object(
    camera: Query<(&Camera, &GlobalTransform), With<WorldCamera>>,
    window: Query<&Window, With<PrimaryWindow>>,
    rig: Res<CameraControl>,
    pointer_over_ui: Res<PointerOverUi>,
    occlusion: Res<PickOcclusion>,
    // The unit pick, earlier in the chain: a unit hover keeps the GameObject halo shut.
    unit_hovered: Res<Hovered>,
    mut hovered: ResMut<HoveredObject>,
    mut picker: ObjectPicker,
    mut sticky: ResMut<StickyPicks>,
) {
    hovered.target = None;
    hovered.guid = None;
    hovered.distance = f32::MAX;
    // The sticky pick drops the moment the pointer is not ours.
    let yielded = rig.is_looking() || pointer_over_ui.0;
    if yielded {
        sticky.object = None;
    }
    let (Ok(cam), Ok(window)) = (camera.single(), window.single()) else {
        return;
    };
    let (_, probe_aim) = picker.probe_aim();
    if yielded {
        return;
    }
    let Some(cursor) = window.cursor_position().or(probe_aim) else {
        return;
    };
    let Ok(ray) = cam.0.viewport_to_world(cam.1, cursor) else {
        return;
    };
    *hovered = picker.pick(
        ray,
        cam,
        window,
        cursor,
        &occlusion,
        &unit_hovered,
        &mut sticky.object,
    );
}

/// The world pick at one point, the reference's `PickAtCursor` (`0x481190`) as the press calls it
/// (`0x481f00`): the world hit, the unit, the GameObject and the cursor's classification of them,
/// as one [`SystemParam`]. The pointer's gates are the caller's: the press reached the world
/// frame, so neither the interface's hover nor a look session in flight blanks it.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct PointPick<'w, 's> {
    camera: Query<'w, 's, (&'static Camera, &'static GlobalTransform), With<WorldCamera>>,
    window: Query<'w, 's, &'static Window, With<PrimaryWindow>>,
    trace: OcclusionTrace<'w, 's>,
    units: UnitPicker<'w, 's>,
    objects: ObjectPicker<'w, 's>,
    plate_hover: Res<'w, crate::vplates::PlateHover>,
    classify: super::cursor_mode::CursorInputs<'w, 's>,
    sticky: ResMut<'w, StickyPicks>,
}

impl PointPick<'_, '_> {
    /// The pick at `at`, in window coordinates; with none, at the window's cursor or the probe's
    /// aim. A plate under the pointer is its unit, as the hover's is: the plate took the press.
    pub(crate) fn at(&mut self, at: Option<Vec2>) -> super::PressPick {
        let plate = self.plate_hover.0;
        self.pick(at, plate)
    }

    /// `PickAtCursor`'s own pick at `at` (`0x481190`), as an arm makes it: the world alone. It
    /// traces the terrain and the world frame's drawn objects (`0x480df0`, `0x480a50`) and asks no
    /// interface frame, so a plate under the cursor is not its unit.
    pub(crate) fn world_at(&mut self, at: Option<Vec2>) -> super::PressPick {
        self.pick(at, None)
    }

    /// The pick at `at`, `plate` the plate whose unit it is.
    fn pick(&mut self, at: Option<Vec2>, plate: Option<Entity>) -> super::PressPick {
        let mut pick = super::PressPick::default();
        let Ok(window) = self.window.single() else {
            return pick;
        };
        let cursor = at
            .or_else(|| window.cursor_position())
            .or(self.objects.probe_aim().1);
        let cam = self.camera.single().ok();
        match (cursor, cam) {
            (Some(cursor), Some(cam)) => {
                if let Ok(ray) = cam.0.viewport_to_world(cam.1, cursor) {
                    pick.occlusion = self.trace.trace(ray);
                    let sticky = &mut *self.sticky;
                    pick.hovered = match plate {
                        Some(entity) => self.units.plate(entity, &mut sticky.unit),
                        None => self
                            .units
                            .pick(ray, pick.occlusion.distance, &mut sticky.unit),
                    };
                    pick.object = self.objects.pick(
                        ray,
                        cam,
                        window,
                        cursor,
                        &pick.occlusion,
                        &pick.hovered,
                        &mut sticky.object,
                    );
                }
            }
            _ => *self.sticky = StickyPicks::default(),
        }
        (pick.cursor, pick.attack_fork) =
            super::cursor_mode::classify(&self.classify, &pick.hovered, &pick.object);
        pick
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::spell::targeting::corpse_fixture as fx;
    use bevy::ecs::system::RunSystemOnce;

    /// The plate hover the pick reads is this frame's walk's: a plate's leave publishes the loss
    /// inside the walk (`0x7cb8b0`), so a pick a frame behind would publish the plate again.
    #[test]
    fn the_pick_reads_the_plate_hover_of_this_frames_walk() {
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        assert!(crate::test_support::runs_before(
            &mut app,
            crate::vplates::seed_plate_hover,
            update_hover
        ));
    }

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

    /// First person hides our own parts (`Visibility::Hidden`), and the posed pick drops them, as
    /// `ShouldRender 0x5ec8f0` refuses the active player then; anyone else's stay the root's call.
    #[test]
    fn our_hidden_parts_leave_the_posed_pick() {
        let mut world = World::new();
        let root = world.spawn_empty().id();
        let rig = world.spawn_empty().id();
        let part = |world: &mut World, drawn| {
            world
                .spawn((
                    ChildOf(root),
                    Mesh3d(Handle::default()),
                    benilla_world::rig_palette::RigPart(rig),
                    drawn,
                ))
                .id()
        };
        let shown = part(&mut world, InheritedVisibility::VISIBLE);
        part(&mut world, InheritedVisibility::HIDDEN);
        let count = move |is_self: bool| {
            move |kids: Query<&Children>,
                  parts: Query<(
                &Mesh3d,
                &benilla_world::rig_palette::RigPart,
                Option<&InheritedVisibility>,
            )>| {
                posed_parts(kids.get(root).expect("the parts"), &parts, is_self).len()
            }
        };
        assert_eq!(world.run_system_once(count(false)).unwrap(), 2);
        assert_eq!(
            world.run_system_once(count(true)).unwrap(),
            1,
            "ours: only the drawn part"
        );
        world.entity_mut(shown).insert(InheritedVisibility::HIDDEN);
        assert_eq!(
            world.run_system_once(count(true)).unwrap(),
            0,
            "first person: nothing of ours to pick"
        );
    }

    /// Under a `sweep` probe the aim moves every frame, and the occlusion trace casts from the
    /// point the object pick reads in the same frame, never the last frame's.
    #[test]
    fn the_occlusion_trace_casts_from_the_frames_probe_aim() {
        use bevy::camera::RenderTargetInfo;
        use bevy::window::WindowResolution;

        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
            avian3d::prelude::PhysicsPlugins::new(bevy::app::PostUpdate),
        ));
        app.init_asset::<Mesh>()
            .init_resource::<benilla_world::collision::ColliderEpoch>()
            .insert_resource(crate::target::hover_probe::ProbeAim::sweep())
            .init_resource::<PickOcclusion>()
            .add_systems(Update, aim_and_trace());
        app.finish();
        app.cleanup();
        let mut camera = Camera::default();
        camera.computed.clip_from_view = Mat4::perspective_infinite_reverse_rh(1.0, 1.0, 0.1);
        camera.computed.target_info = Some(RenderTargetInfo {
            physical_size: UVec2::splat(100),
            scale_factor: 1.0,
        });
        let world = app.world_mut();
        world.spawn((camera, Transform::from_xyz(0.0, 0.0, 10.0), WorldCamera));
        // A cursorless window, as an unattended run's is.
        world.spawn((
            Window {
                resolution: WindowResolution::new(100, 100),
                ..Default::default()
            },
            PrimaryWindow,
        ));
        app.update();
        for frame in 0..6 {
            app.update();
            let world = app.world_mut();
            let aim = world
                .resource::<crate::target::hover_probe::ProbeAim>()
                .now()
                .expect("a sweep aims every frame");
            let (camera, cam_tf) = world
                .query_filtered::<(&Camera, &GlobalTransform), With<WorldCamera>>()
                .single(world)
                .unwrap();
            let want = camera.viewport_to_world(cam_tf, aim).unwrap().direction;
            let got = world.resource::<PickOcclusion>().ray;
            assert!(
                got.is_some_and(|d| d.dot(*want) > 1.0 - 1e-6),
                "frame {frame}: the trace cast along {got:?}, the pick's aim {aim} is along {want:?}"
            );
        }
    }

    /// The world trace tests a static doodad's drawn opaque render mesh, never its collision
    /// hull (`0x69cdb0` queues it in mode 2 under the mask `0x1000114`; mode 2 of `0x7089c0` tests
    /// the skinned submeshes of blend 0). The plaque on Stormwind's cathedral fountain: along the
    /// cursor ray the fountain's hull is at 3.909, nearer than the plaque at 3.943, and its opaque
    /// mesh at 4.029, behind it; so the trace's hit is the mesh, and the plaque picks. A nearer
    /// alpha-keyed batch and an undrawn opaque one do not occlude; terrain behind still does.
    #[test]
    fn a_doodad_occludes_the_pick_by_its_drawn_opaque_mesh_not_its_hull() {
        use avian3d::prelude::{Collider, RigidBody};
        use benilla_formats::{ModelBlend, RenderSubmesh};
        use benilla_world::model_render::{ModelKind, ModelPart};
        use bevy::camera::visibility::SetViewVisibility;
        use bevy::camera::RenderTargetInfo;
        use bevy::window::WindowResolution;

        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
            avian3d::prelude::PhysicsPlugins::new(bevy::app::PostUpdate),
        ));
        app.init_asset::<Mesh>()
            .init_resource::<benilla_world::collision::ColliderEpoch>()
            .init_resource::<crate::target::hover_probe::ProbeAim>()
            .init_resource::<PickOcclusion>();
        app.finish();
        app.cleanup();
        // The camera ten yards up +Z, looking down it, the cursor at the centre.
        let mut camera = Camera::default();
        camera.computed.clip_from_view = Mat4::perspective_infinite_reverse_rh(1.0, 1.0, 0.1);
        camera.computed.target_info = Some(RenderTargetInfo {
            physical_size: UVec2::splat(100),
            scale_factor: 1.0,
        });
        let world = app.world_mut();
        world.spawn((camera, Transform::from_xyz(0.0, 0.0, 10.0), WorldCamera));
        let mut window = Window {
            resolution: WindowResolution::new(100, 100),
            ..Default::default()
        };
        window.set_cursor_position(Some(Vec2::splat(50.0)));
        world.spawn((window, PrimaryWindow));
        // The ray starts on the near plane, 0.1 in front of the camera.
        const EYE: f32 = 9.9;
        // A slab facing the camera `d` yards along the ray: in WoW axes the plane x = 0, which
        // bakes into Bevy's z = 0, moved to z = EYE - d.
        let slab = |blend| {
            std::sync::Arc::new(RenderSubmesh {
                positions: vec![[0.0, -5.0, -5.0], [0.0, 5.0, -5.0], [0.0, 0.0, 5.0]],
                indices: vec![0, 1, 2],
                blend,
                ..Default::default()
            })
        };
        let mut batch = |d: f32, blend, drawn: bool| {
            let mut e = world.spawn((
                benilla_world::interact::PickMesh(slab(blend)),
                GlobalTransform::from_translation(Vec3::new(0.0, 0.0, EYE - d)),
                ViewVisibility::HIDDEN,
                ModelPart {
                    kind: ModelKind::Doodad,
                    blend,
                },
            ));
            // What the spawn marks a static doodad's batch of blend 0 with.
            if blend == ModelBlend::Opaque {
                e.insert(benilla_world::interact::PickOccluderPart);
            }
            if drawn {
                e.get_mut::<ViewVisibility>().unwrap().set_visible();
            }
        };
        batch(4.029, ModelBlend::Opaque, true);
        batch(3.5, ModelBlend::AlphaTest, true);
        batch(3.0, ModelBlend::Opaque, false);
        // The fountain's hull, a static body with no occluder mark, as every doodad hull spawns.
        world.spawn((
            RigidBody::Static,
            Collider::cuboid(4.0, 4.0, 0.2),
            Transform::from_xyz(0.0, 0.0, EYE - 3.909 - 0.1),
        ));
        // Terrain behind it all.
        world.spawn((
            RigidBody::Static,
            Collider::cuboid(40.0, 40.0, 0.2),
            Transform::from_xyz(0.0, 0.0, EYE - 12.0 - 0.1),
            PickOccluder,
        ));
        app.update();
        app.world_mut()
            .run_system_once(update_pick_occlusion)
            .unwrap();
        let distance = app.world().resource::<PickOcclusion>().distance;
        assert!(
            (distance - 4.029).abs() < 1e-3,
            "the fountain's opaque mesh, behind the plaque at 3.943: {distance}"
        );
    }

    /// The picker's inputs with a camera ten yards up the Z axis, looking down it through a
    /// 100-pixel square with the cursor at its centre, over whatever stands at the origin.
    fn pick_world() -> World {
        use bevy::camera::RenderTargetInfo;
        use bevy::window::WindowResolution;

        let mut world = World::new();
        world.init_resource::<CameraControl>();
        world.init_resource::<PointerOverUi>();
        world.insert_resource(PickOcclusion {
            distance: f32::INFINITY,
            point: None,
            ray: None,
        });
        world.init_resource::<crate::vplates::PlateHover>();
        world.init_resource::<Hovered>();
        world.init_resource::<StickyPicks>();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<benilla_world::rig_palette::RigPalettes>();
        world.insert_resource(crate::net::SelfGuid(Some(1)));
        world.init_resource::<crate::spell::PicksSelf>();
        world.init_resource::<crate::spell::CorpsePick>();
        world.init_resource::<crate::spell::targeting::UnitPick>();
        world.insert_resource(crate::net::Reputations::default());
        let mut camera = Camera::default();
        camera.computed.clip_from_view = Mat4::perspective_infinite_reverse_rh(1.0, 1.0, 0.1);
        camera.computed.target_info = Some(RenderTargetInfo {
            physical_size: UVec2::splat(100),
            scale_factor: 1.0,
        });
        world.spawn((
            camera,
            GlobalTransform::from_translation(Vec3::new(0.0, 0.0, 10.0)),
            WorldCamera,
        ));
        let mut window = Window {
            resolution: WindowResolution::new(100, 100),
            ..Default::default()
        };
        window.set_cursor_position(Some(Vec2::splat(50.0)));
        world.spawn((window, PrimaryWindow));
        world
    }

    /// One drawn pick box around the origin, the box fallback's geometry for a partless body.
    fn pick_box(world: &mut World, body: Entity) {
        world.spawn((
            ChildOf(body),
            CreaturePickPart,
            Aabb::from_min_max(Vec3::splat(-1.0), Vec3::splat(1.0)),
            GlobalTransform::default(),
            InheritedVisibility::VISIBLE,
        ));
    }

    /// Our own body joins the unit pick only while [`crate::spell::PicksSelf`] says the word takes
    /// it (`0x480610` at `480638`), here through the box fallback of a body with no skinned parts.
    #[test]
    fn our_body_joins_the_pick_only_while_the_word_takes_it() {
        let mut world = pick_world();
        // Us, at the origin under the cursor, with one drawn pick box.
        let me = world
            .spawn((
                SelfPlayer,
                Guid(1),
                NetEntity {
                    kind: EntityKind::Player,
                    display_id: None,
                    scale: 1.0,
                },
                GlobalTransform::default(),
                InheritedVisibility::VISIBLE,
            ))
            .id();
        pick_box(&mut world, me);
        let hover = |world: &mut World, picks: bool| {
            world.resource_mut::<crate::spell::PicksSelf>().0 = picks;
            world
                .run_system_cached(update_hover)
                .expect("the pick runs");
            world.resource::<Hovered>().target
        };

        assert_eq!(hover(&mut world, false), None, "not ours to pick");
        assert_eq!(hover(&mut world, true), Some(me), "a word that takes us");
        assert_eq!(hover(&mut world, false), None, "and not once it ends");
    }

    /// A corpse is a candidate outside targeting (`0x5c` carries flag `0x40`); while targeting,
    /// only under a word in `0x8600` and only one `0x6e6260` passes, as
    /// [`crate::spell::CorpsePick`] carries the standing word to the picker.
    #[test]
    fn a_corpse_joins_the_pick_as_the_word_admits_it() {
        use crate::spell::targeting::corpse_fixture as fx;

        let hover = |corpse: ObjectStore, word: Option<u16>| {
            let mut world = pick_world();
            world.insert_resource(fx::factions());
            world.init_resource::<crate::spell::SpellTargeting>();
            // Us, a human, away from the ray: the reaction gate's other side.
            world.spawn((
                SelfPlayer,
                Guid(1),
                GlobalTransform::from_translation(Vec3::new(50.0, 0.0, 0.0)),
                fx::caster(1.5),
            ));
            let body = world
                .spawn((
                    Guid(fx::CORPSE),
                    NetEntity {
                        kind: EntityKind::Corpse,
                        display_id: None,
                        scale: 1.0,
                    },
                    GlobalTransform::default(),
                    InheritedVisibility::VISIBLE,
                    corpse,
                ))
                .id();
            pick_box(&mut world, body);
            if let Some(word) = word {
                world.resource_mut::<crate::spell::SpellTargeting>().enter(
                    fx::RESURRECTION,
                    crate::spell::CastCommit::Spell,
                    word,
                );
            }
            world
                .run_system_cached(crate::spell::targeting::publish_corpse_pick)
                .expect("the word publishes");
            world
                .run_system_cached(update_hover)
                .expect("the pick runs");
            let hovered = *world.resource::<Hovered>();
            assert_eq!(hovered.target, None, "a corpse is never the unit slot");
            hovered.corpse.inspect(|&e| assert_eq!(e, body)).is_some()
        };
        let friend = || fx::corpse(fx::HUMAN, false);
        let foe = || fx::corpse(fx::ORC, false);
        let bones = || fx::corpse(fx::HUMAN, true);

        // Not targeting: every corpse, bones and foes included.
        for (corpse, what) in [(friend(), "friend"), (foe(), "foe"), (bones(), "bones")] {
            assert!(hover(corpse, None), "{what} outside targeting");
        }
        // Resurrection's word: a friend's corpse, never a foe's or bones.
        assert!(hover(friend(), Some(0x8000)));
        assert!(!hover(foe(), Some(0x8000)), "a hostile corpse");
        assert!(!hover(bones(), Some(0x8000)), "bones");
        // The enemy bit takes the other side.
        assert!(hover(foe(), Some(0x0200)));
        assert!(!hover(friend(), Some(0x0200)));
        // A heal word sets no flag `0x40`: the corpse is out of the pick, so a unit behind it
        // is reachable.
        assert!(!hover(friend(), Some(0x0002)));
        assert!(!hover(friend(), Some(0x0040)), "a ground word");
    }

    /// A unit or player at depth `z` on the cursor's ray with one drawn pick box, of this store.
    fn unit_at(world: &mut World, guid: u64, z: f32, store: ObjectStore) -> Entity {
        let at = GlobalTransform::from_translation(Vec3::new(0.0, 0.0, z));
        let body = world
            .spawn((
                Guid(guid),
                NetEntity {
                    kind: EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                at,
                InheritedVisibility::VISIBLE,
                store,
            ))
            .id();
        world.spawn((
            ChildOf(body),
            CreaturePickPart,
            Aabb::from_min_max(Vec3::splat(-1.0), Vec3::splat(1.0)),
            at,
            InheritedVisibility::VISIBLE,
        ));
        body
    }

    /// A store of `UNIT_FIELD_FACTIONTEMPLATE` (35), `UNIT_FIELD_FLAGS` (46) and
    /// `UNIT_FIELD_HEALTH` (22), on [`fx::factions`]' rows: template 1 the caster's Human, 2 an Orc.
    fn unit_store(template: u32, flags: u32, health: u32) -> ObjectStore {
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (35, template),
            (46, flags),
            (22, health),
        ]))
    }

    /// A living player of the caster's faction: assistable, not attackable.
    fn friend() -> ObjectStore {
        unit_store(1, 0x8, 100)
    }

    /// A living Orc creature: attackable, not assistable.
    fn foe() -> ObjectStore {
        unit_store(2, 0, 100)
    }

    /// The pick over `word`, the way a frame runs it: the word published, its flags `0x20` and the
    /// unit legs both, then the pick.
    fn pick_under(world: &mut World, word: Option<u16>) -> Hovered {
        let mut targeting = world.resource_mut::<crate::spell::SpellTargeting>();
        match word {
            Some(word) => targeting.enter(fx::RESURRECTION, crate::spell::CastCommit::Spell, word),
            None => targeting.clear(),
        }
        world
            .run_system_cached(crate::spell::targeting::publish_unit_pick)
            .expect("the word publishes");
        world
            .run_system_cached(crate::spell::targeting::publish_picks_self)
            .expect("the self bit publishes");
        world
            .run_system_cached(update_hover)
            .expect("the pick runs");
        *world.resource::<Hovered>()
    }

    /// Us, a human away from the ray, the faction tables, and the units a case puts on it.
    fn unit_pick_world() -> World {
        let mut world = pick_world();
        world.init_resource::<GoPickable>();
        world.insert_resource(fx::factions());
        world.init_resource::<crate::spell::SpellTargeting>();
        world.spawn((
            SelfPlayer,
            Guid(1),
            GlobalTransform::from_translation(Vec3::new(50.0, 0.0, 0.0)),
            fx::caster(1.5),
        ));
        world
    }

    /// `0x480610` while a word stands: a unit the word cannot take is no candidate, so the one
    /// behind it takes the pick, where outside targeting the nearer wins whatever it is. The
    /// front unit is 5 yards nearer the camera than the rear one.
    #[test]
    fn a_unit_the_word_cannot_take_lets_the_pick_through_to_the_one_behind() {
        const HEAL: u16 = 0x0100;
        const ENEMY: u16 = 0x0080;
        const RESURRECTION: u16 = 0x8000;
        const SKINNING: u16 = 0x0402;
        // (word, the front unit it refuses, the rear unit it takes, what stands behind it)
        let cases: [(&str, u16, ObjectStore, ObjectStore); 6] = [
            ("a foe before a friend, under a heal", HEAL, foe(), friend()),
            (
                "a corpse before a friend, under a heal",
                HEAL,
                unit_store(1, 0x8, 0),
                friend(),
            ),
            (
                "a friend before a foe, under an enemy word",
                ENEMY,
                friend(),
                foe(),
            ),
            (
                "the living before a corpse, under Resurrection",
                RESURRECTION,
                friend(),
                unit_store(1, 0x8, 0),
            ),
            (
                "the living before a corpse, under Skinning",
                SKINNING,
                foe(),
                unit_store(2, 0, 0),
            ),
            (
                "a unit immune to players before a friend, under a heal",
                HEAL,
                unit_store(1, 0x108, 100),
                friend(),
            ),
        ];
        for (what, word, front_store, rear_store) in cases {
            let mut world = unit_pick_world();
            let front = unit_at(&mut world, 10, 5.0, front_store);
            let rear = unit_at(&mut world, 11, 0.0, rear_store);
            // Outside targeting the nearer unit wins, whoever it is.
            assert_eq!(
                pick_under(&mut world, None).target,
                Some(front),
                "{what}: outside targeting"
            );
            let hovered = pick_under(&mut world, Some(word));
            assert_eq!(
                hovered.target,
                Some(rear),
                "{what}: the word takes the rear"
            );
            assert_eq!(hovered.guid, Some(11), "{what}");
            assert_eq!(
                pick_under(&mut world, None).target,
                Some(front),
                "{what}: and back outside targeting"
            );
        }
    }

    /// The control of the table above: the same front unit is the pick when the word takes it.
    #[test]
    fn a_unit_the_word_takes_keeps_the_pick_in_front_of_the_one_behind() {
        let cases: [(&str, u16, ObjectStore); 4] = [
            ("a friend under a heal", 0x0100, friend()),
            ("a foe under an enemy word", 0x0080, foe()),
            ("a corpse under Resurrection", 0x8000, unit_store(1, 0x8, 0)),
            ("a foe under a plain unit word", 0x0002, foe()),
        ];
        for (what, word, front_store) in cases {
            let mut world = unit_pick_world();
            let front = unit_at(&mut world, 10, 5.0, front_store);
            let rear_store = if word == 0x8000 {
                unit_store(1, 0x8, 0)
            } else {
                friend()
            };
            unit_at(&mut world, 11, 0.0, rear_store);
            assert_eq!(
                pick_under(&mut world, Some(word)).target,
                Some(front),
                "{what}"
            );
        }
    }

    /// With nothing behind it, a unit the word cannot take leaves the pick empty, which is no
    /// refusal: `IsSelectable` grades the winner only, and there is none.
    #[test]
    fn a_lone_unit_the_word_cannot_take_is_no_pick_at_all() {
        let mut world = unit_pick_world();
        unit_at(&mut world, 10, 0.0, foe());
        let hovered = pick_under(&mut world, Some(0x0100));
        assert_eq!((hovered.target, hovered.guid), (None, None));
        assert!(!hovered.refused);
        assert_eq!(hovered.distance, f32::MAX);
    }

    /// A living Orc creature whose `UNIT_FIELD_FLAGS` are `flags`, with its `UNIT_FIELD_CHARMEDBY`
    /// (10) and `UNIT_FIELD_CREATEDBY` (14) the guids `charmed_by` and `created_by`, 0 for none.
    fn owned(flags: u32, charmed_by: u32, created_by: u32) -> ObjectStore {
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (35, 2),
            (46, flags),
            (22, 100),
            (10, charmed_by),
            (11, 0),
            (14, created_by),
            (15, 0),
        ]))
    }

    /// The word's plain unit bit (`0x0002`), which `0x6e61a0` takes the player for (`0x850e`), and
    /// the enemy bit (`0x0080`), which it does not.
    const SELF_WORD: u16 = 0x0002;
    const ENEMY_WORD: u16 = 0x0080;

    /// `0x480610`, `48064f`-`480676`: a unit flagged `UNIT_FLAG_POSSESSED` whose charmer, else
    /// creator, is the active player is no candidate outside targeting, or while a word without
    /// `0x20` stands, so the pick passes through it to the unit behind. Us is guid 1.
    #[test]
    fn a_unit_you_possess_lets_the_pick_through_to_the_one_behind() {
        let possessed = crate::target::UNIT_FLAG_POSSESSED;
        let cases: [(&str, ObjectStore); 3] = [
            (
                "a Mind Controlled unit, charmed by us",
                owned(possessed, 1, 0),
            ),
            ("an Eye of Kilrogg, created by us", owned(possessed, 0, 1)),
            (
                "charmed by us, created by another: the charmer comes first",
                owned(possessed, 1, 7),
            ),
        ];
        for (what, front_store) in cases {
            let mut world = unit_pick_world();
            let front = unit_at(&mut world, 10, 5.0, front_store);
            let rear = unit_at(&mut world, 11, 0.0, foe());
            for word in [None, Some(ENEMY_WORD)] {
                let hovered = pick_under(&mut world, word);
                assert_eq!(
                    hovered.target,
                    Some(rear),
                    "{what}: the rear, word {word:?}"
                );
                assert_eq!(hovered.guid, Some(11), "{what}: word {word:?}");
            }
            // The word that takes us takes it too: the exclusions are skipped (`48062c`).
            assert_eq!(
                pick_under(&mut world, Some(SELF_WORD)).target,
                Some(front),
                "{what}: while flag 0x20 stands"
            );
            assert_eq!(
                pick_under(&mut world, None).target,
                Some(rear),
                "{what}: and excluded again once the word ends"
            );
        }
    }

    /// With nothing behind it, the unit you possess leaves the pick empty, which is no refusal.
    #[test]
    fn a_lone_unit_you_possess_is_no_pick_at_all() {
        let mut world = unit_pick_world();
        unit_at(
            &mut world,
            10,
            0.0,
            owned(crate::target::UNIT_FLAG_POSSESSED, 1, 0),
        );
        let hovered = pick_under(&mut world, None);
        assert_eq!((hovered.target, hovered.guid), (None, None));
        assert!(!hovered.refused);
    }

    /// The exclusion asks for the flag, then for us: an owned unit that is not possessed, a unit
    /// someone else possesses, and one whose charmer is another even where its creator is us, all
    /// stay candidates.
    #[test]
    fn a_unit_you_do_not_possess_keeps_the_pick() {
        let possessed = crate::target::UNIT_FLAG_POSSESSED;
        let cases: [(&str, ObjectStore); 5] = [
            ("possessed, charmed by another", owned(possessed, 2, 0)),
            ("possessed, created by another", owned(possessed, 0, 2)),
            (
                "possessed, charmed by another, created by us: the charmer comes first",
                owned(possessed, 2, 1),
            ),
            (
                "our own pet, created by us but not possessed",
                owned(0, 0, 1),
            ),
            ("possessed by no one", owned(possessed, 0, 0)),
        ];
        for (what, front_store) in cases {
            let mut world = unit_pick_world();
            let front = unit_at(&mut world, 10, 5.0, front_store);
            unit_at(&mut world, 11, 0.0, foe());
            for word in [None, Some(ENEMY_WORD), Some(SELF_WORD)] {
                assert_eq!(
                    pick_under(&mut world, word).target,
                    Some(front),
                    "{what}: word {word:?}"
                );
            }
        }
    }

    /// `0x481116`: only a word with unit bits (`0x878e`) puts units and players in the pick. A
    /// ground word, a lock word and a bag-item word pick none, so a unit in front of the ground
    /// or the chest is no obstacle, and a unit word does pick them.
    #[test]
    fn only_a_word_with_unit_bits_picks_units() {
        let mut world = unit_pick_world();
        let front = unit_at(&mut world, 10, 5.0, friend());
        for word in [0x0040, 0x0020, 0x4800, 0x0800, 0x0010] {
            let hovered = pick_under(&mut world, Some(word));
            assert_eq!(hovered.target, None, "{word:#06x}");
            assert!(!hovered.refused, "{word:#06x}");
        }
        for word in [0x0002, 0x0100] {
            assert_eq!(
                pick_under(&mut world, Some(word)).target,
                Some(front),
                "{word:#06x}"
            );
        }
    }

    /// While a word stands the grader forks to the targeting arm (`0x4828f4`) and publishes the
    /// pick (`0x4829d4`-`0x4829e0`) without the `IsSelectable` call (`0x482982`), so a
    /// `UNIT_FLAG_NOT_SELECTABLE` unit the word admits is the hover; outside targeting it is
    /// refused. A plain unit word (`0x0002`) asks the filter for no relation (`0x481050`).
    #[test]
    fn while_targeting_the_hover_skips_the_selectable_grade() {
        const NOT_SELECTABLE: u32 = 0x0200_0000;
        let mut world = unit_pick_world();
        let farmer = unit_at(&mut world, 10, 0.0, unit_store(2, NOT_SELECTABLE, 100));
        let outside = pick_under(&mut world, None);
        assert_eq!(
            outside.target, None,
            "outside targeting the grader refuses it"
        );
        assert!(outside.refused);
        let aimed = pick_under(&mut world, Some(0x0002));
        assert_eq!(aimed.target, Some(farmer), "a word takes it ungraded");
        assert_eq!(aimed.guid, Some(10));
        assert!(!aimed.refused);
    }

    /// What the press's pick reads beyond the hover's: the GameObject pick and the cursor's
    /// classifier ([`super::super::cursor_mode::CursorInputs`]).
    fn press_pick_world() -> World {
        let mut world = unit_pick_world();
        world.init_resource::<HoveredObject>();
        world.init_resource::<crate::target::PressPick>();
        world.init_resource::<ButtonInput<MouseButton>>();
        world.init_resource::<ButtonInput<KeyCode>>();
        world.init_resource::<crate::ui_script::InputRecords>();
        world.init_resource::<crate::target::hover_probe::ProbeAim>();
        world.init_resource::<crate::items::Items>();
        world.init_resource::<crate::net::GuidIndex>();
        world.init_resource::<crate::spell::targeting::GoPick>();
        world.init_resource::<crate::go_templates::GameObjectTemplates>();
        world.init_resource::<crate::player::MoverLock>();
        world.init_resource::<crate::player::OwnControlLost>();
        world.init_resource::<crate::player::Approach>();
        world.init_resource::<crate::ui_action::PlayerActions>();
        world.init_resource::<crate::ui_action::LearnedAbilities>();
        world.init_resource::<crate::ui_quest::QuestGiver>();
        world.init_resource::<crate::ui_loot::LootConfig>();
        world.init_resource::<crate::ui_loot::LootLatch>();
        // The world trace's colliders: none.
        world.init_resource::<avian3d::collider_tree::ColliderTrees>();
        world
    }

    /// The press's pick at `at` ([`PointPick`]).
    fn press_at(world: &mut World, at: Vec2) -> crate::target::PressPick {
        world
            .run_system_cached(track_go_pick_parts)
            .expect("the pick set keeps");
        world
            .run_system_cached_with(
                |In(at): In<Option<Vec2>>, mut pick: PointPick| pick.at(at),
                Some(at),
            )
            .expect("the press picks")
    }

    /// The press picks what the word leaves, so the click reaches what the cursor showed: the unit
    /// behind, not the one in front the word refuses.
    #[test]
    fn the_press_picks_what_the_word_left() {
        let mut world = press_pick_world();
        unit_at(&mut world, 10, 5.0, foe());
        let rear = unit_at(&mut world, 11, 0.0, friend());
        pick_under(&mut world, Some(0x0100));
        assert_eq!(
            press_at(&mut world, Vec2::splat(50.0)).hovered.target,
            Some(rear)
        );
    }

    /// The press traces its own pick at the cursor it was queued at (`0x481f00` → `0x481190`), not
    /// the hover a frame left: a unit under the press is picked though the hover was elsewhere,
    /// and a press beside it picks nothing though the hover stood on it.
    #[test]
    fn the_press_picks_what_is_under_the_press_not_the_last_hover() {
        let mut world = press_pick_world();
        let unit = unit_at(&mut world, 10, 0.0, friend());
        let cursor_at = |world: &mut World, at: Vec2| {
            world
                .query::<&mut Window>()
                .single_mut(world)
                .unwrap()
                .set_cursor_position(Some(at));
        };
        // The hover stood off the unit, in the window's corner; the press comes at its centre.
        cursor_at(&mut world, Vec2::splat(2.0));
        assert_eq!(
            pick_under(&mut world, None).target,
            None,
            "the hover is off it"
        );
        assert_eq!(
            press_at(&mut world, Vec2::splat(50.0)).hovered.target,
            Some(unit)
        );
        // And the other way: the hover on the unit, the press in the corner.
        cursor_at(&mut world, Vec2::splat(50.0));
        assert_eq!(pick_under(&mut world, None).target, Some(unit));
        assert_eq!(press_at(&mut world, Vec2::splat(2.0)).hovered.target, None);
    }

    /// A GameObject of `entry` at depth `z` on the cursor's ray, of type 3 (CHEST) with these
    /// `GAMEOBJECT_FLAGS` (field 9), with one drawn pick box.
    fn chest_at(world: &mut World, guid: u64, entry: u32, z: f32, flags: u32) -> Entity {
        use benilla_protocol::field::FIELD_OBJECT_ENTRY;
        let at = GlobalTransform::from_translation(Vec3::new(0.0, 0.0, z));
        let store = ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (2, 0x21),
            (FIELD_OBJECT_ENTRY, entry),
            (9, flags),
            (21, 3),
        ]));
        let go = world.spawn((Guid(guid), at, store)).id();
        let mut part = world.spawn((
            ChildOf(go),
            benilla_world::interact::GoPickPart,
            benilla_world::interact::PickBox,
            Aabb::from_min_max(Vec3::splat(-1.0), Vec3::splat(1.0)),
            at,
            ViewVisibility::HIDDEN,
        ));
        use bevy::camera::visibility::SetViewVisibility;
        part.get_mut::<ViewVisibility>()
            .expect("just spawned")
            .set_visible();
        go
    }

    /// Opening's lock type is 1; an herb's lock is type 2, which it does not open.
    const OPENING: u32 = 3365;

    /// The GameObject pick's world: two chests on the ray, entry 7 locked by lock 55 (type 1, which
    /// [`OPENING`] opens) and entry 8 by lock 56 (type 2), the spells and the templates.
    fn go_pick_world() -> World {
        use benilla_formats::{LockCatalog, LockSlot, SpellDisplay, LOCK_KEY_SKILL};
        let mut world = unit_pick_world();
        world.init_resource::<HoveredObject>();
        world.init_resource::<crate::target::hover_probe::ProbeAim>();
        world.init_resource::<crate::items::Items>();
        world.init_resource::<crate::net::GuidIndex>();
        world.init_resource::<crate::spell::targeting::GoPick>();
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.catalog =
            benilla_formats::SpellCatalog::from_displays(std::collections::HashMap::from([(
                OPENING,
                SpellDisplay {
                    targets: 0x4000,
                    // Effect 0 is `OPEN_LOCK` (33) on LockType 1, as Opening's row has it.
                    effects: [0x21, 0, 0],
                    effect_misc_value: [1, 0, 0],
                    ..SpellDisplay::default()
                },
            )]));
        world.insert_resource(spells);
        let lock = |index| {
            let mut slots = [LockSlot::default(); benilla_formats::MAX_LOCK_SLOTS];
            slots[0] = LockSlot {
                key_type: LOCK_KEY_SKILL,
                index,
                skill: 0,
                action: 5,
            };
            slots
        };
        world.insert_resource(crate::go_templates::Locks(LockCatalog::from_rows([
            (55, lock(1)),
            (56, lock(2)),
        ])));
        let mut templates = crate::go_templates::GameObjectTemplates::default();
        for (entry, lock_id) in [(7, 55), (8, 56)] {
            let mut data = [0; 24];
            data[0] = lock_id;
            templates.insert(entry, 3, "Chest".into(), &data);
        }
        world.insert_resource(templates);
        world
    }

    /// The GameObject pick over `word`, as a frame runs it: the word published, then the pick.
    fn go_pick_under(world: &mut World, word: Option<u16>) -> Option<Entity> {
        let mut targeting = world.resource_mut::<crate::spell::SpellTargeting>();
        match word {
            Some(word) => targeting.enter(OPENING, crate::spell::CastCommit::Spell, word),
            None => targeting.clear(),
        }
        world
            .run_system_cached(crate::spell::targeting::publish_go_pick)
            .expect("the word publishes");
        world
            .run_system_cached(track_go_pick_parts)
            .expect("the pick set keeps");
        world
            .run_system_cached(update_hovered_object)
            .expect("the pick runs");
        world.resource::<HoveredObject>().target
    }

    /// `0x481050` sets the GameObject flag `0x4` while targeting only for a word in `0x4800`
    /// (`0x481101`-`0x48110a`), and `0x480780`'s GameObject arm then admits only an object the
    /// spell's lock test passes (`0x4807fa`-`0x48080c`, `0x6e62e0`, `0x5f8260`). Outside targeting
    /// the nearer object wins, whatever it is. The herb chest is 5 yards nearer the camera.
    #[test]
    fn while_targeting_the_go_pick_takes_only_what_the_word_can_open() {
        let mut world = go_pick_world();
        let herb = chest_at(&mut world, 0xF110_0000_0800_0002, 8, 5.0, 0);
        let chest = chest_at(&mut world, 0xF110_0000_0700_0001, 7, 0.0, 0);
        assert_eq!(
            go_pick_under(&mut world, None),
            Some(herb),
            "outside targeting"
        );
        assert_eq!(
            go_pick_under(&mut world, Some(0x4800)),
            Some(chest),
            "Opening passes over the lock it cannot open"
        );
        for word in [0x0100, 0x0040, 0x0010] {
            assert_eq!(
                go_pick_under(&mut world, Some(word)),
                None,
                "a word without 0x4800 picks no GameObject ({word:#06x})"
            );
        }
        assert_eq!(go_pick_under(&mut world, None), Some(herb), "and back");
    }

    /// `0x5f8260`'s `OPEN_LOCK_ITEM` (59) arm (`0x5f831c`-`0x5f837e`): the cast item named by the
    /// record's guid (`0xceac48`, looked up as an item, `0x5f833a`) opens a lock whose KEY slot
    /// (type 1) indexes its entry, so a key's cursor admits that lock to the pick and no other.
    #[test]
    fn while_targeting_a_key_picks_the_lock_it_fits() {
        use benilla_formats::{LockCatalog, LockSlot, SpellDisplay, LOCK_KEY_ITEM};
        use benilla_protocol::field::FIELD_OBJECT_ENTRY;
        const KEY_SPELL: u32 = 3366;
        const KEY: u64 = 0x4000_0000_0000_0042;
        let pick_with_key = |key_entry: u32| {
            let mut world = go_pick_world();
            world.resource_mut::<crate::ui_action::Spells>().catalog =
                benilla_formats::SpellCatalog::from_displays(std::collections::HashMap::from([(
                    KEY_SPELL,
                    SpellDisplay {
                        effects: [59, 0, 0],
                        ..SpellDisplay::default()
                    },
                )]));
            let mut slots = [LockSlot::default(); benilla_formats::MAX_LOCK_SLOTS];
            slots[0] = LockSlot {
                key_type: LOCK_KEY_ITEM,
                index: 5000,
                skill: 0,
                action: 5,
            };
            world.insert_resource(crate::go_templates::Locks(LockCatalog::from_rows([(
                57, slots,
            )])));
            let mut data = [0; 24];
            data[0] = 57;
            world
                .resource_mut::<crate::go_templates::GameObjectTemplates>()
                .insert(9, 3, "Locked Chest".into(), &data);
            let chest = chest_at(&mut world, 0xF110_0000_0900_0003, 9, 0.0, 0);
            let key = world
                .spawn((
                    Guid(KEY),
                    ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                        (2, 0x3),
                        (FIELD_OBJECT_ENTRY, key_entry),
                    ])),
                ))
                .id();
            world
                .resource_mut::<crate::net::GuidIndex>()
                .0
                .insert(KEY, key);
            world.resource_mut::<crate::spell::SpellTargeting>().enter(
                KEY_SPELL,
                crate::spell::CastCommit::Item {
                    bag_index: 0,
                    slot: 0,
                    entry: key_entry,
                    guid: Some(KEY),
                    spell_index: 0,
                    on_object: None,
                },
                0x4800,
            );
            world
                .run_system_cached(crate::spell::targeting::publish_go_pick)
                .expect("the word publishes");
            world
                .run_system_cached(track_go_pick_parts)
                .expect("the pick set keeps");
            world
                .run_system_cached(update_hovered_object)
                .expect("the pick runs");
            (world.resource::<HoveredObject>().target, chest)
        };
        let (picked, chest) = pick_with_key(5000);
        assert_eq!(picked, Some(chest), "the key fits");
        assert_eq!(pick_with_key(5001).0, None, "another key does not");
    }

    /// While targeting, the grader publishes the pick without the mouseover slot `+0x54`
    /// (`0x4828f4` → `0x4829d4`), so a chest the word admits is the hover even where its
    /// `NO_INTERACT` flag keeps it out outside targeting.
    #[test]
    fn while_targeting_the_go_pick_skips_the_mouseover_slot() {
        let mut world = go_pick_world();
        let chest = chest_at(&mut world, 0xF110_0000_0700_0001, 7, 0.0, 0x10);
        assert_eq!(go_pick_under(&mut world, None), None, "not eligible");
        assert_eq!(go_pick_under(&mut world, Some(0x4800)), Some(chest));
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
