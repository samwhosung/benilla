//! Per-frame motion for streamed units between server packets. A [`Spline`] is a server path
//! sampled at constant speed, from `SMSG_MONSTER_MOVE` or joined mid-path from a create block;
//! [`RemoteMotion`] is another player's flag-driven dead-reckoning, snapped to each new packet,
//! with a jump played out locally under gravity. Poses live in raw WoW space on the components;
//! the `Transform` is derived from them and keeps the renderer's baked scale.

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

/// The Bevy rotation of a wire yaw (radians about WoW +Z): a Bevy +Y yaw with no 180° offset,
/// unlike doodad and WMO placement.
pub(super) fn wire_yaw(orientation: f32) -> Quat {
    Quat::from_rotation_y(orientation)
}

/// The Bevy rotation of a GameObject placement: its `GAMEOBJECT_ROTATION` quaternion, which the
/// reference's render matrix and collision use (`0x5f7910`); `GAMEOBJECT_FACING` (`0x5f9fb0`) is
/// never consulted for placement.
///
/// vmangos fills `rotation2/3` from the facing when the spawn row leaves them zero
/// (`GameObject::UpdateRotationFields`), so a yaw-only row still sends `rot_z(o)`, and a row with
/// a tilt may send a non-unit quat, which `wow_rotation_to_bevy` normalizes. An absent or all-zero
/// quat (a create block folds absent fields to zero) falls back to the facing, since it would
/// normalize to `NaN`.
pub(super) fn gameobject_rotation(quat: Option<[f32; 4]>, orientation: f32) -> Quat {
    match quat {
        Some(q) if q.iter().map(|c| c * c).sum::<f32>() > 0.5 => wow_rotation_to_bevy(q),
        _ => wire_yaw(orientation),
    }
}

/// A spawn [`Transform`] from a raw-WoW position and a resolved rotation ([`wire_yaw`] or
/// [`gameobject_rotation`]).
pub(super) fn pose_transform(position: [f32; 3], rotation: Quat) -> Transform {
    Transform {
        translation: wow_to_bevy(position),
        rotation,
        ..default()
    }
}

/// Writes a raw-WoW pose onto an existing entity, keeping the renderer's baked scale; an entity
/// not yet query-visible on its spawn frame is skipped, as its spawn pose already placed it.
///
/// A wire pose overrides the local facing turn, so it drops any [`DisplayFacing`] and the smoother
/// re-seeds from this pose, as the reference's resync sets the goal and wipes the ring
/// (`0x601020`).
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

/// The WoW yaw of a unit rotation, which is always built by [`Quat::from_rotation_y`].
fn yaw_of(rotation: Quat) -> f32 {
    rotation.to_euler(EulerRot::YXZ).0
}
