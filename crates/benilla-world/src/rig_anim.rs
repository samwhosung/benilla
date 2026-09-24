//! **The rig machinery the renderer needs** — pose storage, the pose post-pass window, the rig
//! world composition, and the free-running global-sequence channels.
//!
//! Split out of `creature_anim` by 1160 stage two, along the line 1163 drew when it re-checked
//! `finalize_rig_worlds` and found it was not gameplay behaviour at all: "its entire input set is
//! rig/transform data, and its only `crate::` references are `billboard`, `rig_palette` and
//! `WorldCamera` — all three engine. **Mis-filing, not behaviour.**" The same is true of the pose
//! evaluator beside it (its only cross-module reference is `billboard`) and of the global-sequence
//! driver (which has none at all).
//!
//! The line between this and what stayed is **machinery versus policy**. Evaluating an
//! `AnimationPlayer` into per-bone locals, composing those into world matrices and palette rows,
//! and running a model's free channels are things a second program must do to put an animated
//! model in a world — so they are engine, and `benilla-worldview` gets them for free. *Which* clip
//! a unit plays, when a mount rider re-seats, when a rig is worth parking — that is a game's
//! business and stayed in `creature_anim`.
//!
//! [`AnimParked`] sits here for the same reason: `creature_anim::lod` decides *when* to park (a
//! policy, reading net entities and the target), but the marker gates the pose evaluator and the
//! billboard joint pass, both of which are here.

use bevy::prelude::*;

mod compose;
mod global_seq;
mod pose;

pub(crate) use compose::seed_rig_rows;
pub use compose::{finalize_rig_worlds, PosePost};
pub use global_seq::GlobalSeqDrive;
pub use pose::{RigAnchor, RigFrame, RigPose};

/// This rig's per-bone pose evaluation is parked: the pose evaluator and the pose
/// post-passes skip it. The sequence clocks, the driver state machine, and the event scanner all
/// keep running — parking turns *sampling* off, nothing else.
#[derive(Component)]
pub struct AnimParked;

/// Wire the rig machinery in. Registered by `WorldPlugins`, so a program with no game still poses
/// and composes the rigs it spawns.
pub fn plugin(app: &mut App) {
    global_seq::plugin(app);
    pose::plugin(app);
    compose::plugin(app);
}
