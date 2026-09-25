//! The movement modes the server grants a unit we do not control: the `SMSG_SPLINE_MOVE_*` state.

use benilla_protocol::SplineMode;
use bevy::prelude::*;

use crate::creature_anim::move_flags;

/// A streamed unit's server-granted movement modes as `MOVEMENTFLAGS` bits: root, water-walk,
/// feather-fall, hover, walk-mode and swim. The reference keeps one flags dword per unit
/// (`CMovement` at `CGUnit+0x9a8`, word `+0x40`); the granted half is kept apart from
/// [`super::RemoteMotion::flags`] because the relay merge rewrites that word from every pose and
/// a creature sends none. Absent means no modes; once inserted it lives as long as the entity, as
/// the reference's word lives as long as the unit (`0x7c4850`).
#[derive(Component, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct UnitMoveModes(pub(crate) u32);

impl UnitMoveModes {
    /// `apply` is the bit's direction; the run/walk opcodes' inversion is folded at the parse
    /// ([`SplineMode::WalkMode`]).
    pub(crate) fn set(&mut self, mode: SplineMode, apply: bool) {
        if apply {
            self.0 |= mode.flag();
        } else {
            self.0 &= !mode.flag();
        }
    }

    /// `MOVEFLAG_ROOT`: the unit cannot move, jump or be splined; the reference's server-position
    /// apply `0x6187a0` refuses while the bit is set (`0x6187c2`).
    pub(crate) fn rooted(self) -> bool {
        self.0 & move_flags::ROOT != 0
    }

    /// `MOVEFLAG_HOVER`: the body rests [`crate::player::HOVER_HEIGHT`] above the floor.
    pub(crate) fn hovering(self) -> bool {
        self.0 & move_flags::HOVER != 0
    }

    /// `MOVEFLAG_WATERWALKING`: the liquid surface counts as walkable ground.
    pub(crate) fn water_walking(self) -> bool {
        self.0 & move_flags::WATER_WALKING != 0
    }
}

// `MOVEFLAG_SAFE_FALL` has no accessor: only our mover (`MoveModes::feather_fall`) and relayed
// players (their own pose flags) fall; a creature rides its spline and never does.

/// What `SetRoot` `0x7c7340` clears from the flags word at apply, as an AND mask: the direction,
/// turn and pitch bits (`0xff`), the `0x8000` latch and the input latches (`0x1f0000`); `ROOT`
/// survives. A one-shot wipe, not a gate: with no direction bits the unit fails the integration
/// test (`0x616e20`, `move_flags::INTEGRATED`) and stops rather than coasting.
pub(crate) const ROOT_APPLY_WIPE: u32 = 0xffe0_7f00;
