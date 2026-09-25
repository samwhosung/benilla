//! The movement modes the server grants a unit we do **not** control — the `SMSG_SPLINE_MOVE_*`
//! family's state.

use benilla_protocol::SplineMode;
use bevy::prelude::*;

use crate::creature_anim::move_flags;

/// **A streamed unit's server-granted movement modes**, as `MOVEMENTFLAGS` bits — root, water-walk,
/// feather-fall, hover, walk-mode and swim.
///
/// This is [`crate::player::state::MoveModes`]' observer twin: the same bits, in the same word, for
/// a body somebody else is driving. It exists because those bits had **nowhere to live** for a unit
/// that is not our mover — a remote player's arrive inside [`super::RemoteMotion::flags`] with each
/// relayed pose, and a creature, which never sends a pose at all, had no flags word whatsoever. So
/// every mode a creature was granted was simply dropped: `crates/benilla-app/src/net/motion/
/// remote.rs` said as much in as many words ("a remote's own granted modes are not modelled yet").
///
/// **One word per unit is the reference's own shape**, not a convenience: `CMovement` is embedded at
/// `CGUnit+0x9a8` and its `+0x40` MOVEMENTFLAGS dword is *the* per-unit movement state, written by
/// the relay merge, the create block, the spline installer and this opcode family alike. We keep
/// the granted-mode half separate from
/// [`super::RemoteMotion::flags`] for one reason: the relay merge re-authors that word wholesale
/// from every pose, and a creature has no poses to be re-authored from.
///
/// Absent component ⇒ no modes granted, which is the overwhelming majority of units; it is inserted
/// on the first `SMSG_SPLINE_MOVE_*` that names the unit and lives as long as the entity, exactly as
/// the reference's word lives as long as the `CGUnit` (`0x7c4850`: `CMovement` is
/// constructed once per unit and nothing else clears it).
#[derive(Component, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct UnitMoveModes(pub(crate) u32);

impl UnitMoveModes {
    /// Grant or revoke one mode. `apply` is always the direction of the bit — the run/walk pair's
    /// inversion against its opcode names is folded at the parse ([`SplineMode::WalkMode`]).
    pub(crate) fn set(&mut self, mode: SplineMode, apply: bool) {
        if apply {
            self.0 |= mode.flag();
        } else {
            self.0 &= !mode.flag();
        }
    }

    /// Rooted (`MOVEFLAG_ROOT`) — this unit **cannot be splined**: the reference's server-position
    /// apply `0x6187a0` refuses outright while the bit is set (`0x6187c2 test ah,0x10`). A rooted
    /// unit cannot translate, cannot jump, cannot be splined.
    pub(crate) fn rooted(self) -> bool {
        self.0 & move_flags::ROOT != 0
    }

    /// Hovering (`MOVEFLAG_HOVER`) — the body rests [`crate::player::HOVER_HEIGHT`] above the floor.
    pub(crate) fn hovering(self) -> bool {
        self.0 & move_flags::HOVER != 0
    }

    /// Water-walking (`MOVEFLAG_WATERWALKING`) — the liquid surface counts as walkable ground.
    pub(crate) fn water_walking(self) -> bool {
        self.0 & move_flags::WATER_WALKING != 0
    }
}

// **`MOVEFLAG_SAFE_FALL` deliberately has no accessor here.** The bit is carried in the word (and
// folded into the animation selector's view like every other), but nothing reads it *from this
// component*, for a structural reason: benilla integrates a fall for exactly two bodies — our own
// mover, whose feather fall is [`crate::player::state::MoveModes::feather_fall`], and a relayed
// player, whose `MOVEFLAG_SAFE_FALL` rides its own poses inside the server-authored merge mask and
// is read straight off [`super::RemoteMotion::flags`] there. A creature, the one body this
// component is really for, never falls: it rides its [`super::Spline`] and its Z is the ground
// clamp's. An accessor with no caller would be a claim that the mode is wired when it is not.

/// **What `SetRoot 0x7c7340` wipes from the flags word at apply**, as a mask to AND with: the four
/// direction bits, the two keyboard turn bits and the two pitch bits (`0xff`), the `0x8000` latch,
/// and the five deferred-input latches (`0x1f0000`) — `and dword ptr [esi+0x40], 0xffe07f00`.
///
/// It is a **one-shot wipe at apply**, not a standing gate, and it is the whole reason a rooted
/// mover stops rather than coasting: with the direction bits gone the unit fails the client's
/// integration gate (`move_flags::INTEGRATED`, `0x616e20 test dword [esi+0x40],0x20ff`) and is not
/// stepped at all. `ROOT` itself is preserved by the mask.
pub(crate) const ROOT_APPLY_WIPE: u32 = 0xffe0_7f00;
