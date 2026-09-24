//! World interaction foundation — *what is the player pointing at?*
//!
//! This is the shared base for every "point at the world" feature: the debug **object inspector**,
//! hover tooltips, the contextual cursor (gear over objects, sword over attackable units),
//! mouseover-targeting, and right-click-to-interact. Built once so we don't build picking +
//! identity twice.
//!
//! The pieces:
//! - [`WorldObject`] — an **identity** component on every pickable world entity (a doodad/WMO model, a
//!   creature, a GameObject): its kind, a human label (model path or unit name), an id, and an optional
//!   detail line. Attached at the spawn sites.
//! - the **ray caster** ([`pick`]) — the pick-geometry declarations ([`PickMesh`] / [`PickBox`]) and
//!   the shared triangle-accurate cast against the **actual resident mesh geometry** (so it works on
//!   colliderless props — most doodads, including the campfire — which a physics raycast misses),
//!   plus the generous inflated pass the mouse pick retries with.
//! - [`MouseoverTarget`] — the resource [`update_mouseover`] fills each frame with the nearest
//!   `WorldObject` under the cursor, for the inspector.
//!
//! The **instruments** built on this base — the dev-chord `I` inspector surface, the cast journal,
//! and the mouseover pick that only ever ran while the inspector was armed — live in
//! `debug_panel::{inspect, journal}` (decision 1160's stage zero). They were here first, and they
//! were what made this module read as the most entangled thing on the engine/game line: between
//! them they named the whole `target::cursor_mode` GO-reaction vocabulary, the spell catalog, the
//! GameObject templates, the name cache and the net writer. What remains here — identity, the
//! ray-caster and the three click gestures — reaches for none of it.
//!
//! The player-facing consumers (`crate::target`) run their picks through [`pick`] unconditionally.
//!
//! **Not every drawn thing has an entity**. The consolidating render lanes — the
//! static merge's blobs and the retained static pass — draw many placements as one thing, and the
//! per-placement entity that used to carry the declaration is gone. So the declaration has two more
//! shapes: [`PickBlob`] (members on the blob entity, for a lane that still draws from one) and
//! [`PickSource`] (the lane answers the ray itself, for one that draws from retained buffers).
//! [`WorldPick`] is the bundle that hands a consumer all three at once — reach for it, not for
//! [`PickParts`] alone, or the answer goes quiet over most of the static world.

use bevy::platform::collections::HashSet;
use bevy::prelude::*;

use crate::model_render::ModelKind;

mod pick;

pub use pick::{
    cast_object_ray, cast_pick_ray, cast_pick_ray_inflated, pick_at_cursor, pick_object_at_cursor,
    ObjectHit, PickBlob, PickBox, PickMember, PickMesh, PickParts, PickSource,
};

pub use pick::{ray_member, ray_mesh_bounds, ray_posed_mesh, RayHit};

/// The identity of a pickable world thing, read by the inspector now and by tooltips/cursor/targeting
/// later. Attached to a thing's renderable mesh entities at spawn.
#[derive(Component, Clone, Debug)]
pub struct WorldObject {
    pub kind: ModelKind,
    /// Primary label — a model path (doodads/WMOs/GameObjects) or a unit name. Shown to the player/dev.
    pub label: String,
    /// Identifier — placement uniqueId, server guid, or display id (`0` if none).
    pub id: u32,
    /// Optional second line of kind-specific detail (e.g. `"emitters: 2"`); shown when non-empty.
    pub detail: String,
}

/// Marker on every pickable **GameObject** part — the GO mouseover pick's population
/// (`target/hover.rs`). Inserted beside the part's `WorldObject { kind: GameObject }` at the
/// attach spawn sites, so building the pick set is an archetype-filtered query over the handful
/// of GO parts on screen instead of a kind-compare over every streamed `WorldObject` row (the
/// doodad/WMO population dwarfs it).
#[derive(Component, Clone, Copy)]
pub struct GoPickPart;

/// The creature twin of [`GoPickPart`]: every pickable unit/player part — the model-less fallback
/// cube included — the unit pick's skinless-fallback population. Doodad/WMO parts carry neither
/// marker.
#[derive(Component, Clone, Copy)]
pub struct CreaturePickPart;

/// A left *select* gesture in the world. Emitted by [`crate::player::control`] on the button's
/// **release**, when the press satisfied the reference's click predicate — under 200 ms whatever the
/// mouse did, or under 800 ms having turned the camera less than 2.25° of yaw / 2.0° of pitch
/// ([`crate::player::camera::PressGesture`]).
///
/// **A drag emits this too.** Orbit and select are independent in the reference: the press engages
/// the camera look immediately and arms this test alongside it, so a fast flick-and-click orbits the
/// camera *and* selects. This doc comment used to assert the opposite as design — *"a left drag
/// engages the orbit and emits nothing"* — and a reporter quoted it back at us with an A/B against
/// the real client proving it wrong (ledger B226).
///
/// It carries no position because the *pick* is latched separately, at the press
/// ([`crate::target::PressPick`]) — never the live hover, which is blank by the time a dragged
/// click lands.
#[derive(Message, Clone, Copy)]
pub struct WorldClick;

/// The right button's context gesture — same arbiter and same predicate as [`WorldClick`], for the
/// button whose drag is the character turn. Vanilla's context action: attack a hostile under the
/// cursor (later: interact/gossip on a friendly).
#[derive(Message, Clone, Copy)]
pub struct WorldRightClick;

/// The right button's **DOWN edge** in the world — emitted at the press, before the click-vs-drag
/// test even starts, whenever the press belongs to the world (in the viewport off the UI, or any
/// press while a look session already owns the hidden cursor). The reference's
/// `CGWorldFrame::OnMouseDown 0x483c40` analogue: ground-targeting's right-click cancel hangs off
/// this edge (`0x492c20`), which fires whether the press
/// becomes a click OR a turn-drag, and consumes nothing (the ref handler returns 0, so the
/// BUTTON2 turn and the release's context click still run).
#[derive(Message, Clone, Copy)]
pub struct WorldRightPress;

/// **The world's pick sources, as one `SystemParam`** — the ECS pick geometry ([`PickParts`]) plus
/// every lane that draws without entities ([`PickSource`]).
///
/// Bundled deliberately. A consumer that reaches for the entity half alone still compiles, still
/// casts, and still returns hits — it just stops seeing most of the static world, silently. That is
/// precisely how the inspector, the hover and `WOW_PICK` went quiet when the consolidating lanes
/// shipped: nothing failed, the answers just went blank over trees and buildings.
/// So the roster of lanes lives HERE, once, and a new consumer gets it by construction.
///
/// The caster itself stays lane-agnostic — it only knows the trait. This is the one place that
/// knows which lanes exist.
#[derive(bevy::ecs::system::SystemParam)]
pub struct WorldPick<'w, 's> {
    parts: PickParts<'w, 's>,
    /// The identity every hit resolves through — including the doodad/WMO parts the entity path
    /// still spawns.
    objects: Query<'w, 's, &'static WorldObject>,
    /// The retained static pass — `None` when `WOW_STATIC_GX=0` leaves every placement on the
    /// entity path. (The static merge needs no entry: its blobs ARE entities, and answer through
    /// [`PickBlob`].)
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

    /// The identified cast against everything drawn — see [`cast_object_ray`].
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

    /// The identified cast through a screen pixel — the mouseover's whole job.
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

/// Registers the world-interaction **foundation**: the identity component's three click messages
/// and the shared ray-caster's parameters. The inspector surface and the cast journal that used to
/// be registered here are instruments, and moved to `debug_panel` in decision 1160's stage zero —
/// along with the mouseover pick, which only ever ran while the inspector was armed.
pub struct InteractPlugin;

impl Plugin for InteractPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<WorldClick>()
            .add_message::<WorldRightClick>()
            .add_message::<WorldRightPress>();
    }
}
