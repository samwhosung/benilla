//! Per-frame motion model for streamed units: server splines + remote-player dead-reckoning.
//!
//! Two motion sources drive an entity's [`Transform`] between the sparse server packets (decisions 0052
//! + 0053):
//! - **[`Spline`]** — a server-authored path, sampled at constant speed by
//!   [`spline::sample_splines`]. Creatures ride these. Two sources, one component: a fresh
//!   `SMSG_MONSTER_MOVE` ([`spline::monster_move_spline`]), and the walk a unit was **already** on when
//!   it streamed into view, joined mid-path from its create block ([`spline::create_spline`], 0708).
//! - **[`RemoteMotion`]** — another player's flag-driven locomotion, integrated by
//!   [`remote::extrapolate_remote_units`]: the client's own dead-reckoning in miniature (extrapolate
//!   from the last reported state, snap to the truth on the next packet). A jump is a **ballistic
//!   event** — the launch played out locally under gravity — not flag-driven walking (decision 0053).
//!
//! The parent [`super`] module owns the wire→ECS bridge that seeds + corrects these from the packet
//! stream; this module owns the integration, split by concern: [`spline`] (the path walk + its
//! terrain re-ground), [`remote`] (dead-reckoning + jump ballistics), [`relay`] (***when*** a relayed
//! move replays — the per-unit fire-time chain), [`facing`] (wire facing
//! resolution + the client-local idle re-face and its [`FacingStep`] shuffle latch). This face keeps
//! the shared pose glue. Poses live canonically in raw WoW space on the components; the `Transform`
//! is derived (translation + placement rotation only — scale, baked by the renderer, is preserved).

use benilla_assets::coords::{wow_rotation_to_bevy, wow_to_bevy};
use bevy::prelude::*;

mod facing;
mod modes;
mod relay;
mod remote;
mod spline;
#[cfg(test)]
mod tests;

pub(crate) use facing::drive_display_facing;
pub(in crate::net) use facing::resolve_facing;
pub(in crate::net) use facing::DisplayFacing;
pub(crate) use facing::FacingStep;
pub(crate) use modes::UnitMoveModes;
pub(in crate::net) use modes::ROOT_APPLY_WIPE;
pub(crate) use relay::{PendingMove, RelayMove};
pub(crate) use remote::jump_seed;
pub(crate) use remote::RemoteMotion;
pub(in crate::net) use remote::{apply_move, arrival_snap, trace_relay, RelayOutcome};
pub(super) use remote::{drain_pending_moves, extrapolate_remote_units};
pub(super) use spline::{
    create_spline, ground_clamp_creatures, mark_swimming_creatures, monster_move_spline,
    sample_splines, trace_create_spline, trace_move_snap,
};
pub(crate) use spline::{ground_derived, grounded_y};
pub(crate) use spline::{CreatureSwimming, GroundClamped, Spline, SplineStopped};

/// The Bevy rotation of a wire **yaw** — a mover's orientation, in radians about WoW +Z. It maps
/// straight to a Bevy +Y yaw under `wow_to_bevy` (`coords`' `yaw_conjugates_through_the_basis_map`)
/// — no 180° offset, unlike doodad/WMO *placement* rotation.
pub(super) fn wire_yaw(orientation: f32) -> Quat {
    Quat::from_rotation_y(orientation)
}

/// The Bevy rotation of a **GameObject** placement: its `GAMEOBJECT_ROTATION` quaternion, not its
/// `GAMEOBJECT_FACING` yaw.
///
/// The reference builds a GO's render matrix from that quaternion — `0x5f7910` (the every-frame
/// slot-13 Animate) composes `GetPosition` → gx-rotate by the 4 floats at GO-fields `+0x10` →
/// gx-scale, and the same 4×4 (`obj+0x218`) is what its collision reads. `GAMEOBJECT_FACING` is a
/// *separate* accessor (`0x5f9fb0`) that the placement never consults.
///
/// A yaw is the special case here, not a simplification of it: vmangos copies `rotation0/1` from the
/// spawn row verbatim and fills `rotation2/3` from the facing when the row leaves them zero
/// (`GameObject::UpdateRotationFields`), so a row that authors no `rotation0/1` sends
/// `(0, 0, sin(o/2), cos(o/2))` — and that quaternion **is** `rot_z(o)` exactly. 54 747 of its
/// 56 632 live spawn rows are that case and place bit-identically either way. The 1 885 that do not (1 410
/// authoring a real tilt in `rotation0/1`, 475 a yaw that disagrees with their own `orientation`
/// column; 1 411 of them on a visible display) are exactly what a yaw-only placement gets wrong —
/// and it misreads as a *position* bug, because an M2's geometry sits off its own origin: a signpost
/// pointer's plank is 3.3 yd above the origin and 0.3–2.1 yd out, so dropping a 70° tilt swings that
/// plank 4.3 yd from a spawn point that never moved (decision 1459, bug B89).
///
/// Falls back to the facing when the wire carried no quaternion at all, and when what it carried has
/// no length to normalize — a create block folds absent fields to zero, and an all-zero quat would
/// normalize to `NaN` and blank the object. Length is otherwise *not* assumed: one shipped row
/// authors a tilt with `rotation2/3` zero, so the server fills the yaw pair on top of it and sends a
/// non-unit quat; `wow_rotation_to_bevy` normalizes.
pub(super) fn gameobject_rotation(quat: Option<[f32; 4]>, orientation: f32) -> Quat {
    match quat {
        Some(q) if q.iter().map(|c| c * c).sum::<f32>() > 0.5 => wow_rotation_to_bevy(q),
        _ => wire_yaw(orientation),
    }
}

/// A full [`Transform`] for a freshly-spawned entity from a raw-WoW position + an already-resolved
/// placement rotation ([`wire_yaw`] for a mover, [`gameobject_rotation`] for a GameObject).
pub(super) fn pose_transform(position: [f32; 3], rotation: Quat) -> Transform {
    Transform {
        translation: wow_to_bevy(position),
        rotation,
        ..default()
    }
}

/// Update an existing entity's translation + rotation from a raw-WoW pose, **preserving its scale**
/// (the renderer bakes per-display model scale into the transform; a move must not reset it). The
/// entity may not be query-visible the very frame it spawned — then the move is simply skipped (the
/// spawn pose already placed it, and the next move lands once commands flush).
///
/// **A wire pose is authoritative over the client-local facing turn**, so it drops any
/// [`DisplayFacing`]: the smoother re-seeds its raw facing and its history ring from the pose
/// written here, which is the client's spawn/resync shape (`0x601020` sets the goal and wipes the
/// ring). Doing it here rather than at the call sites is deliberate — a future pose writer cannot
/// forget it and leave a unit swinging back to a heading the server has since moved it off.
pub(super) fn write_pose(
    commands: &mut Commands,
    transforms: &mut Query<&mut Transform>,
    e: Entity,
    position: [f32; 3],
    rotation: Quat,
) {
    if let Ok(mut t) = transforms.get_mut(e) {
        t.translation = wow_to_bevy(position);
        t.rotation = rotation;
        commands.entity(e).remove::<DisplayFacing>();
    }
}

/// Recover the WoW yaw from a unit [`Transform`] rotation (always built as [`Quat::from_rotation_y`]).
fn yaw_of(rotation: Quat) -> f32 {
    rotation.to_euler(EulerRot::YXZ).0
}
