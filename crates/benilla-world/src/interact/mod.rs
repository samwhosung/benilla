//! What the player is pointing at: the identity of every pickable world thing ([`WorldObject`]),
//! the world click gestures, and the ray caster ([`pick`]), which casts against resident geometry
//! so colliderless props that a physics ray misses still pick.
//!
//! The consolidated render lanes draw many placements with no entity per placement, so a pick
//! declaration takes three shapes: [`PickMesh`] or [`PickBox`] on an entity, [`PickBlob`] members
//! on a blob entity, and a [`PickSource`] lane that answers the ray itself.

use bevy::platform::collections::HashSet;
use bevy::prelude::*;

use crate::model_render::ModelKind;

mod pick;

pub use pick::{
    cast_object_ray, cast_pick_ray, cast_pick_ray_inflated, occludes_pick, pick_at_cursor,
    pick_object_at_cursor, ObjectHit, PickBlob, PickBox, PickMember, PickMesh, PickParts,
    PickSource,
};

pub(crate) use pick::{ray_aabb, world_aabb};
pub use pick::{ray_member, ray_mesh_bounds, ray_posed_mesh, RayHit};

/// Marks a static doodad's drawn batch that occludes the mouse pick ([`occludes_pick`]): an
/// entity part of blend 0, so [`DoodadOccluders`] walks these alone, never every model part.
#[derive(Component, Clone, Copy)]
pub struct PickOccluderPart;

impl PickOccluderPart {
    /// The marker for a static doodad's batch of `blend`, `None` for any other.
    pub(crate) fn of(blend: benilla_formats::ModelBlend) -> Option<Self> {
        (blend == benilla_formats::ModelBlend::Opaque).then_some(Self)
    }
}

/// The identity of a pickable world thing, attached to its renderable mesh entities at spawn.
#[derive(Component, Clone, Debug)]
pub struct WorldObject {
    pub kind: ModelKind,
    /// A model path (doodads, WMOs, GameObjects) or a unit name.
    pub label: String,
    /// Placement `uniqueId`, server guid or display id; `0` if none.
    pub id: u32,
    /// An optional kind-specific second line, shown when non-empty.
    pub detail: String,
}

/// Marks every pickable GameObject part, so the GameObject hover's pick set is an archetype query
/// rather than a scan of every `WorldObject`.
#[derive(Component, Clone, Copy)]
pub struct GoPickPart;

/// Marks every pickable unit or player part, the model-less fallback cube included: the unit
/// pick's skinless-fallback set. Doodad and WMO parts carry neither marker.
#[derive(Component, Clone, Copy)]
pub struct CreaturePickPart;

/// A left select gesture, sent on release when the press met the reference's click predicate:
/// under 200 ms, or under 800 ms having turned the camera less than 2.25° of yaw and 2.0° of
/// pitch. A drag that meets it selects too, as the reference arms the click beside the camera look.
/// It carries no position: the pick is latched at the press (`target::PressPick`).
#[derive(Message, Clone, Copy)]
pub struct WorldClick;

/// The right button's context gesture, on the same predicate as [`WorldClick`].
#[derive(Message, Clone, Copy)]
pub struct WorldRightClick;

/// The right button's press in the world (off the UI, or any press while a look holds the hidden
/// cursor), sent before the click test starts, as the reference's `CGWorldFrame::OnMouseDown`
/// (`0x483c40`), where ground targeting's right-click cancel and the repair-mode reset hang
/// (`0x492c20`). It consumes nothing, so the turn and the release's context click still run.
#[derive(Message, Clone, Copy)]
pub struct WorldRightPress;

/// The world's pick sources as one `SystemParam`: the entity pick geometry ([`PickParts`]) and
/// every lane that draws without entities ([`PickSource`]). Use this, not [`PickParts`] alone,
/// which still returns hits but misses most of the static world.
#[derive(bevy::ecs::system::SystemParam)]
pub struct WorldPick<'w, 's> {
    parts: PickParts<'w, 's>,
    /// The identity every entity hit resolves through.
    objects: Query<'w, 's, &'static WorldObject>,
    /// The retained static pass; `None` when `WOW_STATIC_GX=0` keeps every placement on the entity
    /// path. The static merge's blobs are entities and answer through [`PickBlob`].
    gx: Option<Res<'w, crate::static_gx::StaticGx>>,
}

impl WorldPick<'_, '_> {
    /// This frame's entity-less sources, in the shape [`cast_object_ray`] takes.
    fn sources(&self) -> Vec<&dyn PickSource> {
        self.gx
            .as_deref()
            .map(|gx| gx as &dyn PickSource)
            .into_iter()
            .collect()
    }

    /// The identified cast against everything drawn ([`cast_object_ray`]).
    pub fn cast(&self, ray: Ray3d, pickable: &HashSet<Entity>, all_hits: bool) -> Vec<ObjectHit> {
        cast_object_ray(
            ray,
            pickable,
            &self.parts,
            &self.objects,
            &self.sources(),
            all_hits,
        )
    }

    /// The identified cast through a screen pixel.
    pub fn at_cursor(
        &self,
        cursor: Vec2,
        camera: &Camera,
        cam_tf: &GlobalTransform,
        pickable: &HashSet<Entity>,
    ) -> Option<ObjectHit> {
        pick_object_at_cursor(
            cursor,
            camera,
            cam_tf,
            pickable,
            &self.parts,
            &self.objects,
            &self.sources(),
        )
    }
}

/// The static doodads' drawn batches that occlude the mouse pick ([`occludes_pick`]), in every
/// lane that draws them: entity parts, the static merge's blobs and the retained pass. Only what
/// the last render drew: `0x7089c0` skips a model the scene has not animated this frame
/// (`0x708bd9`, instance `+0x40` against the scene's frame `+0x10`), and the scene animates what
/// it collects to draw. The collision hulls are no part of it.
#[derive(bevy::ecs::system::SystemParam)]
pub struct DoodadOccluders<'w, 's> {
    #[allow(clippy::type_complexity)] // one query of the part's pick inputs
    parts: Query<
        'w,
        's,
        (
            &'static PickMesh,
            &'static GlobalTransform,
            &'static ViewVisibility,
            Option<&'static bevy::camera::primitives::Aabb>,
        ),
        With<PickOccluderPart>,
    >,
    #[allow(clippy::type_complexity)] // one query of the blob's pick inputs
    blobs: Query<
        'w,
        's,
        (
            &'static PickBlob,
            &'static GlobalTransform,
            &'static ViewVisibility,
            Option<&'static bevy::camera::primitives::Aabb>,
        ),
    >,
    gx: Option<Res<'w, crate::static_gx::StaticGx>>,
}

impl DoodadOccluders<'_, '_> {
    /// The distance along `ray` to the nearest occluding doodad batch.
    pub fn nearest(&self, ray: Ray3d) -> Option<f32> {
        let (origin, dir) = (ray.origin, *ray.direction);
        let mut best: Option<f32> = None;
        let beyond = |best: Option<f32>, entry: Option<f32>| match entry {
            None => true,
            Some(t) => best.is_some_and(|b| t > b),
        };
        for (mesh, gt, vis, aabb) in &self.parts {
            if !vis.get() {
                continue;
            }
            if aabb.is_some_and(|b| beyond(best, pick::ray_mesh_bounds(origin, dir, b, gt))) {
                continue;
            }
            if let Some(hit) = pick::ray_member_at(&mesh.0, gt, ray) {
                best = Some(best.map_or(hit.distance, |b| b.min(hit.distance)));
            }
        }
        for (blob, gt, vis, aabb) in &self.blobs {
            if !vis.get()
                || aabb.is_some_and(|b| beyond(best, pick::ray_mesh_bounds(origin, dir, b, gt)))
            {
                continue;
            }
            for m in blob.0.iter() {
                if !occludes_pick(&m.object, &m.geometry) {
                    continue;
                }
                if let Some(hit) = pick::ray_member(&m.geometry, m.transform, ray) {
                    best = Some(best.map_or(hit.distance, |b| b.min(hit.distance)));
                }
            }
        }
        if let Some(d) = self.gx.as_deref().and_then(|gx| gx.nearest_occluder(ray)) {
            best = Some(best.map_or(d, |b| b.min(d)));
        }
        best
    }
}

/// Registers the three world click messages.
pub struct InteractPlugin;

impl Plugin for InteractPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<WorldClick>()
            .add_message::<WorldRightClick>()
            .add_message::<WorldRightPress>();
    }
}
