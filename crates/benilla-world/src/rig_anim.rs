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

/// A scene's own clock in seconds, on each model root that stands in a scene that keeps one: a
/// `<Model>` widget's private `CM2Scene` (`CSimpleModel+0x314`, `0x76cfc0`), zeroed by its
/// constructor (`0x706ff0`) and advanced only by the widget's `OnUpdate` (`0x76d7f0`), which the
/// visible-frame pump calls, so a hidden pane's scene stands still. A model with none runs on the
/// world's clock. Read by the global-sequence channels ([`GlobalSeqDrive`]) and the material lanes
/// (`doodad_anim`), each against its own anchor: `clock − attach` for a global sequence
/// (`CM2Model+0x68`), `clock − lo` for a sequence band (`0x714530`).
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct SceneClock(pub f64);

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
