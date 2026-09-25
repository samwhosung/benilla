//! The mouseover pick — which unit is under the cursor, recomputed each frame into
//! [`super::Hovered`] the way the real client finds it (`CGWorldFrame` pick `0x481190` →
//! `0x7089c0`): a **broad phase** ray-vs-sphere on the current animation's bounds, then a
//! **narrow phase** ray-vs-triangle against the unit's **posed render mesh**, with the
//! reference's generous +1-model-unit halo retry when nothing hits exactly. The selection /
//! interaction story that consumes the verdict lives in the [`super`] module doc.

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

/// Trace this frame's **world-occlusion distance** (the reference's `CWorld::Intersect` leg of the
/// scene trace `0x480df0`): one
/// physics ray from the cursor through the [`PickOccluder`] set (terrain, the WMO walk-bake faces
/// ≈ the byte-decoded `0x84` reject-mask, static doodad hulls). Both object picks below run
/// **unbounded** and post-compare against it — the reference discards the object hit iff the world
/// hit is *strictly* nearer (`0x480eb4`: tie keeps the object). No cursor / mouselook →
/// `INFINITY` (the picks bail on their own guards). The occluder set deliberately excludes net
/// entities (units, GameObject hulls, transports) — they are the *object* trace; a chest must not
/// occlude itself.
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
        // Default (terrain/doodads) + the WMO *walk* bake — the mask that reaches every
        // `PickOccluder`-marked collider; the marker picks the exact occluder set within it.
        &benilla_world::collision::WorldCollision::body_filter(),
        &|e| occluders.contains(e),
    ) {
        occlusion.distance = hit.distance;
        occlusion.point = Some(ray.origin + *ray.direction * hit.distance);
    }
}

/// Every model instance whose parts are `unit`'s pick geometry: **the body, everything it wears,
/// and its mount** (the mount leg is 0441's).
///
/// This is the reference's chained-model set. `0x480d90` registers a pick candidate by walking the
/// object's CM2 **attachment tree** — `[model+0x1dc]` list head → `[+0x1e4]` sibling, the same two
/// fields the dress and paperdoll lanes walk — and calling the registrar `0x713cb0` on *each* model
/// it finds, all under one candidate node. So the ray tests the held axe's own triangles and the
/// hit comes back as the unit holding it.
///
/// Takes the worn roots as a plain slice rather than the `HeldAttached` that supplies them, so the
/// rule is testable without building one. `pub(crate)` for the **census**
/// ([`crate::capture::probes`]'s `pick=` column), which must count what the picker will actually
/// test and not a second opinion about it.
pub(crate) fn pick_model_roots(
    unit: Entity,
    worn: &[Option<Entity>],
    mount: Option<Entity>,
) -> impl Iterator<Item = Entity> + use<'_> {
    std::iter::once(unit)
        .chain(worn.iter().flatten().copied())
        .chain(mount)
}

/// **Where every model's skeleton is right now** — the owned palette rows and the per-part link
/// that indexes them, as one [`SystemParam`].
///
/// They are two halves of one lookup ([`update_hover`]'s `palette_of` reads both, in that order,
/// and neither has ever been wanted alone) and they are bundled because the picker sits exactly on
/// Bevy's 16-param function-system ceiling — the seat this freed went to the `IsSelectable` grader,
/// which has to run *inside* the pick rather than after it so [`Hovered`] is never
/// published in its ungraded state.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct PickPose<'w, 's> {
    /// The owned palette table: the picker reads the same world-space matrices the
    /// vertex stage skins with, straight from the CPU rows.
    palettes: Res<'w, benilla_world::rig_palette::RigPalettes>,
    rigs: Query<'w, 's, &'static benilla_world::rig_palette::RigSkin>,
}

/// Recompute the unit under the cursor each frame — the real client's two-phase pick (the
/// resolve `0x7089c0`): **broad** = the cursor ray vs the *current animation's*
/// bounds sphere (world-placed + world-scaled, no pad); **pass 1** = the ray vs the unit's **posed
/// render mesh** (each drawn vertex skinned through the live joint pose, per-triangle), nearest
/// world-distance hit wins; **pass 2, only when pass 1 hit nothing anywhere** (the mouse pick's
/// generous retry — never LoS/spell traces): the same posed mesh with every vertex displaced
/// **+1 model-unit along its skinned normal** (a ~1-yd halo, fattest where the body is widest),
/// resolved by the reference's priority ladder — last frame's pick sticks (anti-flicker), else an
/// alive unit beats a dead one even when farther, ties by distance. A unit with no skinned parts
/// (cube fallback) keeps the interim render-mesh AABB test, competing at pass-1 level. Inert while
/// mouse-looking (cursor hidden) or over the dev UI.
///
/// **A unit's pick geometry includes what it wears**: every model chained to the
/// body — held weapons, the helm, the shoulders, a mount — is its own pass-1/pass-2 candidate
/// resolving to the same unit, because the reference registers the whole CM2 attachment tree into
/// the pick scene under one candidate node (`0x480d90` walking `[model+0x1dc]`/`[+0x1e4]`).
#[allow(clippy::type_complexity)]
pub(super) fn update_hover(
    camera: Query<(&Camera, &GlobalTransform), With<WorldCamera>>,
    window: Query<&Window, With<PrimaryWindow>>,
    rig: Res<CameraControl>,
    pointer_over_ui: Res<PointerOverUi>,
    // The frame's world-occlusion distance: the final pick is discarded iff the world hit is
    // strictly nearer (behind terrain/WMO/doodad geometry — the post-hoc compare below).
    occlusion: Res<PickOcclusion>,
    // The unit whose plate the pointer is inside (last frame's layout — the plate drive runs
    // after this chain), published by the plate widgets themselves.
    plate_hover: Res<crate::vplates::PlateHover>,
    mut hovered: ResMut<Hovered>,
    mesh_assets: Res<Assets<Mesh>>,
    pose: PickPose,
    // `IsSelectable`'s second clause reads the active player's guid — see the grader at the publish.
    self_guid: Res<crate::net::SelfGuid>,
    // Last frame's pick, for pass 2's sticky-hover (the reference's anti-flicker cache: the previous
    // pick outranks everything in the halo retry, so the hover doesn't strobe between two units).
    mut last_pick: Local<Option<Entity>>,
    // Unit roots: pose + scale, the playing animation (for its bounds sphere), the descriptor store
    // (alive-vs-dead sets the pass-2 priority), the part children — and whether the body is drawn
    // at all. **You cannot click what is not in the scene**: a body the
    // exterior-scene election sent to pass 2 never reaches the reference's draw list, so it takes
    // no mouseover, no sword cursor and no click. This lane used to ray-test every unit root with a
    // `Guid` and never ask, which is how the director could still target Tanaris mobs through a
    // sealed cavern ceiling after their models had correctly stopped drawing.
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
            // What this unit WEARS — the spawned root per attach slot. Each is a chained model
            // and therefore pick geometry of its wearer (see the system doc); read here rather
            // than by a query of its own because this system sits at Bevy's 16-param ceiling.
            Option<&HeldAttached>,
        ),
        (With<Guid>, Without<SelfPlayer>),
    >,
    // A mounted unit's mount-child part children: the mount's geometry joins the
    // unit's pick set — the reference draws mount + rider as one clickable unit.
    child_sets: Query<&Children>,
    // A part child's mesh + its palette-rig link (the rig its vertices pose through).
    parts: Query<(&Mesh3d, &benilla_world::rig_palette::RigPart)>,
    // Fallback path: a unit's pickable mesh children, by [`CreaturePickPart`] (stamped beside the
    // part's `WorldObject` at the attach sites, fallback cube included — the archetype filter that
    // replaced a per-frame kind-compare over every streamed row) — model-local `Aabb`, world
    // transform, and the link to the streamed parent (whose `Guid` we resolve the hit to).
    meshes: Query<
        (
            &ChildOf,
            &Aabb,
            &GlobalTransform,
            Option<&InheritedVisibility>,
        ),
        With<CreaturePickPart>,
    >,
    // The pick's guid resolution — **and its kind**, which the skinless fallback below needs to
    // apply the same eligibility rule the faithful path states inline (a corpse
    // renders, and renders as a character body with pickable parts, but it is not a unit and a
    // click must never send `CMSG_SET_SELECTION` for it).
    units: Query<(&Guid, &NetEntity), Without<SelfPlayer>>,
) {
    hovered.target = None;
    hovered.guid = None;
    hovered.corpse = None;
    hovered.corpse_guid = None;
    hovered.distance = f32::MAX;
    hovered.refused = false;
    // **The plate publishes the mouseover, and it does so THROUGH the UI gate below, not around
    // it.** A plate is mouse-enabled UI (2148): with the pointer inside one, the UI pointer pass
    // owns the cursor, `PointerOverUi` is true, and the world pick correctly stands down — so this
    // has to be read before that early return or hovering a plate would clear the mouseover it is
    // supposed to set. That is the reference's own arrangement: the plate's OnEnter (`0x7cb850`)
    // publishes `[0xb4e2c8]` directly, with none of the world hover's grading, and the frame walk
    // stops there — no world pick behind it. Freelook still wins, because there the plates have
    // handed the mouse back (`0x60f830`).
    if !rig.is_looking() {
        if let Some(entity) = plate_hover.0 {
            hovered.target = Some(entity);
            hovered.guid = units.get(entity).ok().map(|(g, _)| g.0);
            hovered.distance = 0.0; // topmost UI — it beats any world GameObject at a tie
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

    // ── The faithful path: collect broad-sphere-passing candidates, then pass 1 / pass 2 ────────
    // A pass-2-eligible candidate: the unit, its pass-2 priority (alive 3 / dead 2 — the lootable
    // refinement awaits loot state), its parts' mesh ids, and its posed joint palette.
    let mut candidates: Vec<(Entity, u8, Vec<AssetId<Mesh>>, Vec<Mat4>)> = Vec::new();
    // Units the faithful path *owns* (they have skinned parts): excluded from the AABB fallback
    // even when the broad phase rejects them — the reference wouldn't click them there either.
    let mut faithful: HashSet<Entity> = HashSet::new();
    for (entity, gt, net, anims, drv, store, children, mount_child, drawn, held) in &roots {
        // Units, players — and **corpses**. A corpse is a skinned character body
        // like any other, so it belongs in this pass, not in a fourth picker duplicating it; the
        // reference picks every CGObject in one trace and switches on type at the end. Which slot
        // of [`Hovered`] the winner lands in is decided at the publish below, by kind.
        if !matches!(
            net.kind,
            EntityKind::Unit | EntityKind::Player | EntityKind::Corpse
        ) {
            continue;
        }
        // Not drawn ⇒ not clickable (see the query's note). Deliberately BEFORE `faithful.insert`
        // below, so an undrawn unit is not claimed by the faithful path either — otherwise it
        // would merely be excluded from the AABB fallback and stay unpickable by accident rather
        // than by rule.
        if !drawn.is_none_or(|v| v.get()) {
            continue;
        }
        let Some(children) = children else { continue };
        // The models this unit WEARS, each its own pick candidate below. The reference registers
        // every model **chained** to the object, not just its body: `0x480d90` walks the CM2
        // attachment tree (`[model+0x1dc]` list head → `[+0x1e4]` sibling — the same two fields
        // the dress and paperdoll lanes walk) and registers each child model into the pick scene
        // under the SAME candidate node, so a ray that strikes the axe resolves to the unit
        // holding it. Ours tested the body alone,
        // which is why a Naxxramas weapon mob — an `InvisibleStalker` body that draws nothing,
        // whose entire visible self is the weapon in its hand — could only be targeted through
        // its name plate.
        let worn = held.map_or(&[][..], |h| h.spawned_slots().as_slice());
        // Ownership probe only — no collect yet. The `skinned` Vec used to be built HERE, for
        // every drawn unit in the scene, ahead of the few-FLOP broad phase that rejects nearly
        // all of them (1473 §3's hover row); the survivors alone pay it now, below. Membership
        // in `faithful` must still be decided pre-reject — a broad-phase-rejected skinned unit
        // stays excluded from the AABB fallback by RULE, not by accident.
        if !children.iter().any(|c| parts.contains(c)) && worn.iter().all(Option::is_none) {
            continue; // static/cube unit → the AABB fallback below
        }
        faithful.insert(entity);
        // Broad phase: the playing sequence's bounds sphere, world-placed + world-scaled (uniform
        // scale — the root bakes `OBJECT_FIELD_SCALE_X`). An unknown clip / unauthored radius passes
        // through (the reference falls back to the header sphere; permissive is the safe direction).
        // A MOUNTED unit passes through too: the rider's Mount-pose sphere doesn't cover the
        // mount's body (the reference recomputes the pick radius from the mount's box — a P2
        // refinement).
        let clip = match (anims, drv) {
            (Some(a), Some(d)) => d.active_anim().and_then(|id| a.find(id)),
            _ => None,
        };
        if let Some(c) = clip.filter(|c| c.bounds_radius > 0.0 && mount_child.is_none()) {
            let centre = gt.transform_point(c.bounds_center);
            let radius = c.bounds_radius * gt.scale().max_element();
            // Ray-sphere: reject when the ray's closest approach to the centre exceeds the radius
            // (dir is normalized; a centre behind the origin projects to t=0, keeping units we
            // stand inside clickable).
            let along = (centre - origin).dot(dir).max(0.0);
            if (centre - origin - dir * along).length_squared() > radius * radius {
                continue;
            }
        }
        // The posed joint palette (parts of one skeleton share the rig): the same
        // world-from-bind-pose matrices GPU skinning applies, read from the owned palette rows
        // (last frame's propagated pose, exactly what the previous
        // joint-GlobalTransform read gave).
        let palette_of =
            |sk: &[(&Mesh3d, &benilla_world::rig_palette::RigPart)]| -> Option<Vec<Mat4>> {
                sk.first().and_then(|(_, part)| {
                    let rig = pose.rigs.get(part.0).ok()?;
                    pose.palettes.world_palette(rig.slot, rig.bones() as usize)
                })
            };
        // The halo ladder (alive 3 / dead 2), with the corpse rung no longer guessed (1723's open
        // question): `0x480c90`'s arm at `0x480cec` scores a corpse **2 when
        // `CORPSE_DYNFLAG_LOOTABLE`, else 0** — the same rung as a dead unit when there is
        // something to take, and the floor otherwise. 1723 guessed a flat 1; this replaces it.
        //
        // The rung matters less than it looks: **pass 1 sorts on distance alone**, so a corpse
        // can neither steal a click from a unit standing in front of it nor lose one it is in
        // front of. Priority decides only the inflated pass-2 retry. Still not read off
        // `unit_is_dead()` — a corpse descriptor has no UNIT block, so that accessor answers
        // "alive" for one.
        let priority = if net.kind == EntityKind::Corpse {
            u8::from(store.is_some_and(|s| s.0.corpse_lootable())) * 2
        } else if store.is_some_and(|s| s.0.unit_is_dead()) {
            2
        } else {
            3
        };
        // **One candidate per model instance, every one of them resolving to this unit** — the
        // reference's own shape (one candidate node; every chained model registered under it).
        // The body contributes nothing when it has no parts, which is not a reason to stop: a
        // trigger creature's whole visible self is what it wears. An attached item rides a rigid
        // one-frame palette of its own (`RigRider`, 1609), so it reads here exactly like a body —
        // its parts, its rig, its pose — and so does a mount child.
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

    // Pass 1 — the exact posed render mesh, pure nearest-wins (the reference's first pass).
    // UNBOUNDED, like the reference: the object trace never carries the world distance; occlusion
    // is one strict compare on the final result (below).
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

    // ── Fallback for skinless units only: the model-bounds box test (pre-RE interim) ────────────
    // A unit's parts are separate meshes; testing each and keeping the closest parent is equivalent
    // to testing the union. `dir` is normalized, so `t` is a world distance like the hits above.
    for (child_of, aabb, gt, drawn) in &meshes {
        // …and the same rule on the skinless fallback: a part inherits its root's pass-2 verdict.
        if !drawn.is_none_or(|v| v.get()) {
            continue;
        }
        let parent = child_of.parent();
        if faithful.contains(&parent) {
            continue; // posed-mesh-tested above
        }
        // The same eligibility the faithful path states at its head — and **a bone pile lives
        // here**: the skeletal corpse model ships without a skeleton, so it never
        // reaches the posed-mesh pass and this box test is the only thing that can ever pick it.
        // The self player is still excluded (its query filter), and so is everything that is not a
        // body: a hit resolves to a `Guid` the click will act on, and the kind decides which slot
        // of [`Hovered`] receives it.
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

    // Pass 2 — nothing exactly hit: the reference's generous retry (mouse-pick only). Same
    // candidates, every vertex displaced +1 model-unit along its skinned normal — a ~1-yd halo
    // around the drawn body. Accept by the priority ladder, not pure distance: last frame's pick
    // sticks (anti-flicker), else higher priority wins even when farther, ties by distance.
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

    // The occlusion verdict — the reference's single strict compare (`0x480df0` @ `0x480eb4`):
    // the object hit is discarded iff the world hit is STRICTLY nearer (a tie keeps the object).
    if best.is_some_and(|(t, _)| limit < t) {
        best = None;
    }
    // **Switch on type at the end**, the reference's own shape: the one pick lands in the unit slot
    // or the corpse slot, never both. Everything downstream that means "unit" reads `target` and so
    // stays right by construction; the corpse legs read `corpse`.
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
        // Set on EVERY arm, the refusal included — see [`Hovered::refused`].
        hovered.distance = t;
    }
    *last_pick = best.map(|(_, e)| e);
}

/// **The hover grader's `IsSelectable` verdict** — `0x4828d0`, the step between the pick and the
/// mouseover publish:
///
/// ```text
/// 48297d  mov  eax,[esi]              ; the picked object's vtable
/// 482982  call [eax+0x54]             ; slot 21 = 0x60be60 IsSelectable — the SAME predicate
/// 482985  test eax,eax                ;   SetSelection runs, reached there via the +0x58 thunk
/// 482987  je   0x4829ed               ; FALSE -> 0x482090(0,0) + ResetCursor 0x523d30
/// ```
///
/// So the refusal is **total, and it is one rule**: the mouseover globals `[0xb4e2c8]/[0xb4e2cc]`
/// are cleared (their only two writers image-wide live in the publisher `0x492890`, which that jump
/// skips), the cursor is reset, and the type dispatch that would have chosen an Interact/Attack
/// cursor never runs. No tooltip, no cursor, no brighten and no click, out of one predicate rather
/// than four.
///
/// **It is a grader, not a filter, and the difference is observable.** A census over the pick itself
/// (`0x480a50`, `0x480d90`, `0x713cb0`, `0x481190`, `0x480df0`) finds **zero** reads of
/// `[obj+0x110]`/`[+0xa0]` — positive control: four such reads inside `CanAttack` — so the flagged
/// unit is registered as a candidate and *wins* the trace before anything looks at it. That is why
/// the caller records [`Hovered::refused`] and keeps the pick's distance instead of skipping the
/// candidate: a trigger stalker in front of a chest takes the pick, and the chest behind it must not
/// inherit the mouseover.
///
/// Applied to the **unit** arms only. A corpse reaches `0x482982` too, but slot 21 there is
/// `CGCorpse_C`'s, not `CGUnit_C`'s — the `xor eax,eax` base stub sits at slot **22** (`+0x58`),
/// which is what makes a non-unit unselectable for `SetSelection`, and is not what this call goes
/// through.
///
/// A **plate** hover is a deliberate non-case: the reference publishes it straight from
/// `0x7cb869 call 0x492890` with no grader at all, so a plate on a flagged unit *would* tooltip —
/// but the per-tick plate gate `0x60f600` refuses the same units one step earlier
/// (`0x60f622`/`0x60f628`, unconditional, re-run every OnUpdate), which `crate::vplates` already
/// models. There is no reachable state where the two disagree.
///
/// Run at the publish rather than as a system of its own, deliberately. The reference really does
/// pick and *then* grade, but a second Bevy system means [`Hovered`] is briefly readable in its
/// ungraded state by anything that forgot to order after `TargetUpdate` — a one-frame tooltip pop
/// for a unit that must never have one. Written once, already graded, that state does not exist.
///
/// Decision 2060.
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

/// Recompute the **GameObject** under the cursor each frame into [`HoveredObject`]:
/// a mesh-accurate ray pick against GameObject parts *only*, reusing the inspector's picker
/// (the resident-geometry caster of decision 0857, which hits the colliderless props a physics ray
/// misses and the `RENDER_WORLD`-only static forms Bevy's `MeshRayCast` lost at 0834). Kept
/// separate from [`update_hover`]'s unit pick because a GameObject is usable-but-not-*selected*:
/// this drives the Interact cursor and the right-click USE, never selection. Inert while
/// mouse-looking (cursor hidden) or over the dev UI, exactly like the unit pick. Cheap — only the
/// handful of GO parts on screen are in the pick set.
///
/// **Two passes, like the unit pick** (GameObjects are the same type-1 candidates
/// as units in the reference's resolve `0x7089c0`): **pass 1** =
/// the exact resident mesh, pure nearest-wins; **pass 2, only when pass 1 hit nothing anywhere**
/// (the mouse pick's generous retry): the same geometry with every vertex displaced +1 model-unit
/// along its authored normal — a ~1-yd halo, which is what makes a wispy herb clickable *around*
/// its leaves instead of only pixel-on-texture. "Nothing anywhere" spans the unit pick too (one
/// resolve in the reference): a frame where [`Hovered`] holds any unit — exact hit or its own
/// halo — never opens the GO halo, because the pass-2 accept ladder ranks every unit (alive 3 /
/// dead 2) above every GameObject (highlightable 1 / else 0). Within the GO halo the same ladder
/// applies: last frame's pick sticks (anti-flicker), else higher priority wins even when farther,
/// ties by distance. Both passes stay world-occlusion-clamped. (Residual, same slice as the
/// eligibility note below: the reference's sticky slot is cross-type, so a stuck GO pick can
/// outrank a unit's halo there; our split picks give the unit that frame instead.)
/// The GO picker's own state, bundled — the pick-set cache with the stream edges that rebuild
/// it, plus the sticky-hover memory — one [`SystemParam`] because [`update_hovered_object`]
/// sits at Bevy's 16-param function-system ceiling. See the maintenance comment at the top of
/// that system for the cache contract.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct GoPickSet<'w, 's> {
    added: Query<'w, 's, (), Added<GoPickPart>>,
    removed: RemovedComponents<'w, 's, GoPickPart>,
    cache: Local<'s, bevy::platform::collections::HashSet<Entity>>,
    /// Last frame's GO pick, for pass 2's sticky-hover (the reference's anti-flicker cache, by
    /// net entity — the same rung the unit picker keeps).
    last_pick: Local<'s, Option<Entity>>,
    /// Every pickable billboard card, for [`net_entity_of`]'s first hop. Bundled here rather than
    /// taken as its own `SystemParam` because [`update_hovered_object`] is at Bevy's 16-param
    /// function-system ceiling.
    cards: Query<'w, 's, &'static BillboardCard>,
    /// The headless probe's frame counter and its say-once report ([`super::hover_probe`], 2250).
    /// `Local`s: in a player run the probe is unarmed, so these are one `u64` and one `None` that
    /// nothing ever reads.
    frame: Local<'s, u64>,
    report: Local<'s, super::hover_probe::ProbeReport>,
    /// `WOW_HOVER_PROBE=lock`'s held target (2255) — the object the probe rests its aim on for the
    /// remainder of the run.
    lock: Local<'s, super::hover_probe::LockedAim>,
}

/// The number of `ChildOf` hops [`net_entity_of`] will climb before giving up — a malformed-data
/// guard, not a real depth. The deepest real chain is a card on a nested joint (bone hierarchies
/// in the shipped corpus are single digits deep), so this is orders of margin.
const NET_WALK_HOPS: usize = 64;

/// A picked GameObject part → the **net entity** that carries its `Guid` (the object the mouseover
/// publishes, the cursor classifies and the right-click USEs).
///
/// Two shapes reach here and they attach differently. An ordinary mesh part is a direct child of
/// the net entity — one `ChildOf` hop. A **billboard card is a world ROOT** (a card
/// writes an absolute world transform and lives at the root/identity), so it has no `ChildOf` edge
/// at all: its link to the model is [`BillboardCard::follows`] — the joint it rides on a rigged
/// host, the mirror anchor under the net entity otherwise — and the climb continues from there.
///
/// Before the card hop existed a card hit resolved to the **card**, which carries no `Guid`, so
/// [`update_hovered_object`] bailed and published the **null** mouseover: no cursor, no tooltip and
/// a dead right-click over exactly the screen area the card covers. The Lightwell found it (B169's
/// second half) — its light shaft is a lock-Z card standing 4.5 yd out of a 1.09 yd bowl, so the
/// card swallowed nearly every ray and only the sliver of bowl around it ever answered.
///
/// The reference has no such split to reconcile: the resolve `0x7089c0`'s narrow phase walks the
/// ONE model's render batches — the actual visible RENDER MESH, posed to the current animation,
/// billboard batch included, skinned through the live bone matrices — so
/// a hit on the shaft **is** a hit on the GameObject, which is what this restores.
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

/// The state the GameObject gates read beside the object's own store, bundled because this system
/// sits at Bevy's 16-`SystemParam` ceiling: the ask-once template cache (GENERIC's eligibility is
/// its `data[1]`, decision 0762; MEETINGSTONE's is its `data[2]`), the faction catalog behind the
/// eligibility faction term, and the live meeting-stone queue — benilla's
/// `[0xb72038]`, the other half of MEETINGSTONE(23)'s own highlightable predicate.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct GoGateInputs<'w> {
    pub(crate) templates: Res<'w, crate::go_templates::GameObjectTemplates>,
    pub(crate) factions: Option<Res<'w, super::ring::Factions>>,
    /// `Option` because a headless/net-less build mounts no UI dialog verbs; absent reads as the
    /// reference's zero-initialized `[0xb72038]`, i.e. not queued.
    pub(crate) stone: Option<Res<'w, crate::ui_dialog_verbs::MeetingStone>>,
}

impl GoGateInputs<'_> {
    /// `[0xb72038]` — the area we are queued at, `0` when we are not (or when there is no VM).
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
    // The unit pick's verdict — [`update_hover`] runs earlier in the target chain. Any unit hover
    // means pass 1 + the unit halo already own the mouseover, so the GO pass 2 stays shut.
    unit_hovered: Res<Hovered>,
    mut hovered: ResMut<HoveredObject>,
    // Every pickable GameObject part carries [`GoPickPart`] (stamped beside its
    // `WorldObject { kind: GameObject }` at the attach sites; units/doodads/WMOs never enter this
    // query — units have their own posed-mesh pick).
    go_parts: Query<Entity, With<GoPickPart>>,
    child_of: Query<&ChildOf>,
    guids: Query<&Guid>,
    stores: Query<&ObjectStore>,
    // The three state reads the GameObject gates need beside the object's own store, as one
    // param (the 16-`SystemParam` ceiling) — see [`GoGateInputs`].
    go_gate: GoGateInputs,
    self_q: Query<&ObjectStore, With<crate::net::SelfPlayer>>,
    parts: PickParts,
    // The picker's own state, one bundled param ([`GoPickSet`] — the fn sits at Bevy's
    // 16-param ceiling): the pick-set cache, its stream edges, and the sticky-hover memory.
    mut cache: GoPickSet,
) {
    *cache.frame = cache.frame.wrapping_add(1);
    let added_parts = &cache.added;
    let removed_parts = &mut cache.removed;
    let pickable = &mut *cache.cache;
    let last_pick = &mut *cache.last_pick;
    let cards = &cache.cards;
    // The hit part → its guid-bearing net entity ([`net_entity_of`]: the card hop, then the climb).
    let resolve_net = |e: Entity| net_entity_of(e, cards, &guids, &child_of);
    // The pick set (bevy's `HashSet` — the caster's type, not the `std` one this module uses
    // elsewhere), rebuilt on GO-part stream edges ONLY — the `WaterIndex` shape. It used to be
    // collected from every streamed GO part on every cursor frame (ChildOf hop + store probe +
    // hash insert each); membership only actually changes when GO models spawn or despawn, and
    // a GO's type id never changes after create. Maintained FIRST, ahead of every early return:
    // an `Added` edge is only visible on the frame it happens (the run advances this system's
    // change tick even through a `return`), so a rebuild parked behind the cursor gates would
    // silently drop parts streamed in while the pointer sat on UI. A despawned entry between
    // edges self-filters at the caster (`parts.get` on a dead entity misses).
    //
    // A transport-family GO (TRANSPORT 11 / MAP_OBJECT 14 / MO_TRANSPORT 15) never joins the
    // set: in the reference it has no pick geometry at all — the GO model resolver `0x5f80e0`
    // returns 0 for these three types (they render via the WMO/spline path, not an M2) and their
    // strategy's interaction predicates are constant-false (w2c1 + go-render-gate + the +0x14
    // vtable dump). Concretely: a ship's hull must not swallow the pick — an NPC on deck stays
    // hoverable through the railing, and the hull shows no gear or tooltip.
    if removed_parts.read().next().is_some() || !added_parts.is_empty() {
        pickable.clear();
        pickable.extend(go_parts.iter().filter(|&e| {
            // Through the same resolver the hit uses, so a card and its mesh siblings can never
            // disagree about which GameObject they belong to (a transport's card would otherwise
            // stay in the set while its meshes were filtered out).
            !stores
                .get(resolve_net(e))
                .is_ok_and(|s| matches!(s.0.gameobject_type_id(), 11 | 14 | 15))
        }));
    }
    hovered.target = None;
    hovered.guid = None;
    hovered.distance = f32::MAX;
    // The sticky-hover cache drops the moment the pointer is not ours, exactly as before — the
    // probe's aim below must not change *when* that happens.
    let yielded = rig.is_looking() || pointer_over_ui.0;
    if yielded {
        *last_pick = None;
    }
    let (Ok((camera, cam_tf)), Ok(window)) = (camera.single(), window.single()) else {
        return;
    };
    // The aim. A person's pointer always wins; the probe (2250) answers only for a window that has
    // no cursor at all, which is every automated run — see [`super::hover_probe`].
    //
    // **Resolved and PUBLISHED ahead of the yield below** (2255), because the UI mouse feed reads
    // the published aim as its pointer: a frame that returned early without publishing would take
    // the pointer off the UI, drop `PointerOverUi`, and unlatch the very gate that skipped the
    // pick — the probe would flicker the interface on and off instead of resting on it.
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

    // The probe's census (2250): once a second, where the pickable GameObjects actually ARE on
    // screen. Without it a headless "no hit" is unreadable — it cannot tell a pick fault from a
    // camera that is simply not looking at the thing, which is the single question a person with a
    // mouse never has to ask.
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

    // Pass 1 — the exact resident geometry, pure nearest-wins (priority-independent).
    let mut best: Option<(f32, Entity)> =
        benilla_world::interact::cast_pick_ray(ray, pickable, &parts, false)
            .into_iter()
            .next()
            .map(|(e, h)| (h.distance, e));

    // Pass 2 — nothing exactly hit anywhere (no GO, and no unit either — the doc above): the
    // generous retry over the normal-inflated geometry, accepted by the priority ladder.
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
                // The classify priority (`0x480c90`): highlightable GameObject 1, else 0. A store
                // that hasn't streamed reads highlightable — the eligibility gate's own
                // permissive default, so a fresh spawn isn't a dead zone for its first frames.
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

    // The occlusion verdict (`0x480df0` @ `0x480eb4`): discard the GO hit iff the world hit is
    // STRICTLY nearer — a tie keeps the object.
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
    // **The mouseover-eligibility gate** (`[obj->vtbl+0x54]` at `0x482982`): an
    // ineligible object publishes the NULL mouseover, so it gets no tooltip, no +64 brighten and no
    // cursor — nothing. Applied here, at the one place the GO mouseover is published, so all three
    // consumers fall out together exactly as they do in the reference (which reaches `0x492890`,
    // `0x52aa20` and `0x4945e0` only past this gate).
    //
    // KNOWN RESIDUAL: the reference nulls the **whole** mouseover, so a unit standing behind an
    // ineligible portcullis is not hoverable through it either. benilla picks unit and GameObject
    // separately and arbitrates by distance, so here the unit behind it stays hoverable. Narrower
    // than the old behaviour (which tooltipped the portcullis itself) and documented rather than
    // silently accepted; closing it wants the single-pick arbitration, which is its own slice.
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
            // `lock` takes its target here and nowhere else: past the eligibility gate, so the
            // probe holds something that actually publishes a mouseover rather than the first
            // lamp-post the grid grazed. The hit point is re-expressed in the PART's frame so the
            // aim survives the camera settling and the object moving (see [`LockedAim`]).
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

    /// **A billboard card's hit belongs to its GameObject** — bug B169's second half, the Lightwell
    /// you could see but not click (2026-08-25).
    ///
    /// A card is a world ROOT, so the old one-`ChildOf`-hop resolve answered with the card itself;
    /// the card carries no `Guid`, so `update_hovered_object` bailed and published the null
    /// mouseover — no cursor, no tooltip, a dead right-click — over the whole screen area the card
    /// covers. On `G_HolyLightWell.m2` that is a 4.5 yd shaft standing out of a 1.09 yd bowl, so
    /// nearly every ray landed on the dead card and only the ring of bowl around it answered:
    /// *"very hard to find the right position, and even when I see the cog nothing happens"*.
    ///
    /// Both card shapes are covered, because `spawn_billboard_part` picks between them on whether
    /// the host is rigged: following the **mirror anchor** (a direct child of the net entity) and
    /// following a **joint** (nested under the model root, so the climb is more than one hop).
    #[test]
    fn a_billboard_cards_hit_resolves_to_its_gameobject() {
        let mut world = World::new();
        let net = world.spawn(Guid(0xdead_beef)).id();
        // An ordinary mesh part: the direct child the old one-hop resolve was written for.
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

    /// **A unit's pick geometry is every model chained to it** — decision 1658, the Naxxramas
    /// weapon mobs (`.go xyz 2729.28 -3159.80 267.54 533`).
    ///
    /// "Unholy Axe" is an `InvisibleStalker` body — 80 bones, 135 animations, zero vertices —
    /// holding a real axe on its right-hand attachment. Enumerating only the body left the unit
    /// with no pick candidate at all, so the model could not be hovered or clicked and the name
    /// plate was the only way to select it. The reference registers each chained model under the
    /// same candidate node, which is what makes the axe itself the clickable surface.
    #[test]
    fn a_units_pick_geometry_is_every_model_chained_to_it() {
        let unit = Entity::from_raw_u32(1).unwrap();
        let main_hand = Entity::from_raw_u32(2).unwrap();
        let helm = Entity::from_raw_u32(3).unwrap();
        let mount = Entity::from_raw_u32(4).unwrap();

        let bare: Vec<_> = pick_model_roots(unit, &[], None).collect();
        assert_eq!(bare, vec![unit], "a unit wearing nothing is just its body");

        // The empty slots between the filled ones are the real `HeldAttached` shape (main/off/
        // ranged/helm/shL/shR/ammo/quiver), and must not become candidates.
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

        // The case that was broken: the body draws nothing, so the axe is the whole hit surface.
        let trigger: Vec<_> = pick_model_roots(unit, &worn[..1], None).collect();
        assert!(
            trigger.contains(&main_hand),
            "a body with no geometry of its own still offers what it holds"
        );
    }
}
