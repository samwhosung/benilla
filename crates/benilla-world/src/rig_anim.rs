//! The rig machinery the renderer needs: pose storage, the pose post-pass window, the rig world
//! composition and the free-running global-sequence channels. Which clip a unit plays and when a
//! rig parks are the game's policy (`creature_anim`); [`AnimParked`] lives here because it gates
//! the pose evaluator and the billboard joint pass.

use bevy::prelude::*;

mod compose;
mod global_seq;
mod pose;

pub(crate) use compose::seed_rig_rows;
pub use compose::{finalize_rig_worlds, PosePost};
pub use global_seq::GlobalSeqDrive;
pub use pose::{RigAnchor, RigFrame, RigPose};

/// Parks this rig's per-bone pose evaluation: the evaluator and the pose post-passes skip it,
/// while the sequence clocks, the driver state machine and the event scanner keep running.
#[derive(Component)]
pub struct AnimParked;

/// Registers the rig machinery; `WorldPlugins` adds it, so a program with no game still poses its
/// rigs.
pub fn plugin(app: &mut App) {
    global_seq::plugin(app);
    pose::plugin(app);
    compose::plugin(app);
}
