//! A body standing in the world: what the engine needs to know about a unit, and nothing about
//! where it came from. The game puts [`WorldUnit`] on every unit and [`ViewerUnit`] on the one the
//! eye belongs to.
//!
//! State `WorldUnit` at the spawn, like `Transform`, when the body must be live the same frame:
//! the wire reconciler (`entities::publish_world_units`) runs right after the wire drain, a frame
//! late for anything spawned later. Its fields copy facts the game owns (`NetEntity::scale`,
//! `CollisionHeight`), refreshed on change.

use bevy::prelude::*;

/// A unit body the world can act on: it wades, takes ground shade, claims a WMO room and holds a
/// rig palette slot. Every field is an engine fact; `benilla-protocol` is not a dependency here.
#[derive(Component, Clone, Copy, Debug)]
pub struct WorldUnit {
    /// Whether this body displaces water (a ripple ring and a wake): a creature or a player does,
    /// a chest or a spell's ground anchor does not. A field, so no spawn can leave it unstated.
    pub wades: bool,
    /// The instance's model scale, as the ripple ring's radius input.
    pub scale: f32,
    /// The collision cylinder's height in yards, the ring's other input. Before the display
    /// resolves it is the reference's constructor default, never `0.0`, at which the body swims on
    /// dry land.
    pub height: f32,
    /// The whole-object model-space box the world may cull this body by: from inside a sealed WMO
    /// room the reference never submits an outdoor object (`crate::exterior_cull`). The game
    /// supplies the idle animation's authored CAaBox, unscaled; a body whose model has not resolved
    /// is culled as a degenerate box at its origin, not admitted. `None` is not the world's to
    /// decide: the transport, whose root `Visibility` its own tick writes.
    pub bound: Option<bevy::camera::primitives::Aabb>,
}

/// The viewer's own body; a marker because every use is a query filter.
#[derive(Component)]
pub struct ViewerUnit;
