//! The three unit-relationship predicates the reference shares across systems, may I attack,
//! interact with or help this unit, as the store-only entry points every consumer calls:
//!
//! - `CanAttack 0x606980` forwards to [`super::ring::can_attack_from_player`],
//! - `CanInteract 0x6067f0` forwards to [`super::ring::can_interact_from_player`],
//! - `CanAssist 0x6066f0` is derived here, on [`ring_reaction`].
//!
//! The first two live in [`super::ring`] beside the reaction directions they turn on:
//! `0x6061e0(this = player)` answers a reputation faction with the at-war bit,
//! `0x6061e0(this = unit)` with the standing, and the two often disagree.

use benilla_protocol::messages::{ObjectType, OwnerFallback};

use crate::net::{ObjectStore, Reputations};

use super::{ring_reaction, Factions};

/// `OBJECT_FIELD_TYPE` bit 4, the reference's own player test (`0x606984` in `CanAttack`,
/// `0x6067fc` in `CanInteract`), read off the store the rest of the predicate walks.
///
/// Relies on the create block carrying field 2: vmangos sends every non-zero field, and a player's
/// is `0x19`. A miss only skips `CanAttack`'s ghost refusal, which the both-player-controlled arm
/// makes anyway; `/reaction` prints the field beside the `NetEntity` kind.
fn is_player_object(store: Option<&ObjectStore>) -> bool {
    store.and_then(|s| s.0.object_type()) == Some(ObjectType::Player)
}

/// `CanAttack 0x606980`, the one attackability question of the world cursor's sword
/// (`0x48269a`), the combat flash, the TAB scan's filter 3, `UnitCanAttack` and hostile spell
/// targeting. Its mixed arm is `UnitReaction(player → target) < 4`, which answers a reputation
/// faction with the at-war bit, never the standing, so a not-at-war neutral faction (Cenarion
/// Circle) is not attackable.
pub(crate) fn can_attack(
    store: Option<&ObjectStore>,
    factions: Option<&Factions>,
    reputations: &Reputations,
    self_store: Option<&ObjectStore>,
) -> bool {
    super::ring::can_attack_from_player(
        factions,
        reputations,
        store,
        self_store,
        is_player_object(store),
    )
}

/// `CanInteract 0x6067f0`, may I take a service from this unit: the world-cursor classifier runs it
/// at `0x482310` (through `CanInteractNow 0x606880`) to choose between the NPC service ladder and
/// the loot, skin and attack block.
pub(crate) fn can_interact(
    store: Option<&ObjectStore>,
    factions: Option<&Factions>,
    reputations: &Reputations,
    self_store: Option<&ObjectStore>,
) -> bool {
    super::ring::can_interact_from_player(factions, reputations, store, self_store)
}

/// `UNIT_FLAG_NOT_SELECTABLE`, bit 25 (vmangos `UnitDefines.h:570`): `CanAssist`'s first refusal
/// and one of `CanAttack`'s.
const UNIT_FLAG_NOT_SELECTABLE: u32 = 1 << 25;

/// `IsSelectable`, CGUnit_C's vtable slot 21 (`0x60be60`): `UNIT_FLAG_NOT_SELECTABLE` clear, or the
/// unit's `UNIT_FIELD_CREATEDBY` is the active player, so your own flagged totems and traps stay
/// selectable. Every vtable but CGUnit_C's and CGPlayer_C's has the `xor eax,eax` stub
/// (`0x469fe0`) there, so a GameObject, item or corpse never becomes the selection.
///
/// `None` passes: `SetSelection 0x493540` makes the call (`0x4935ee`) only on an object the
/// manager resolved (`0x4935c8`), which keeps an out-of-range party member selectable. A store
/// whose `OBJECT_FIELD_TYPE` has not streamed passes the same way.
pub(crate) fn is_selectable(store: Option<&ObjectStore>, self_guid: Option<u64>) -> bool {
    let Some(store) = store else {
        return true; // no resolved object: the reference skips the call (`0x4935c8`)
    };
    // Only CGUnit_C and CGPlayer_C override slot 21.
    if !matches!(
        store.0.object_type(),
        None | Some(ObjectType::Unit) | Some(ObjectType::Player)
    ) {
        return false;
    }
    store.0.unit_flags() & UNIT_FLAG_NOT_SELECTABLE == 0
        || (self_guid.is_some() && store.0.unit_created_by() == self_guid)
}

/// `UNIT_FLAG_PVP`, bit 12 (vmangos `UnitDefines.h:557`), what `IsPvP 0x605ff0` tests on the
/// unit's owner.
const UNIT_FLAG_PVP: u32 = 0x1000;
/// `UNIT_FIELD_FLAGS` bit 3, player-controlled (vmangos `UnitDefines.h:548`), the bit
/// [`ring_reaction`]'s duel leg selects on.
const UNIT_FLAG_PLAYER_CONTROLLED: u32 = 0x8;

/// `CanAssist 0x6066f0` (Lua `UnitCanAssist` `0x516bb0`, registered at `.data 0x8504c8`), the
/// unit-level gate of `UnitBuff` ([`crate::ui_aura::buffs_visible_on`]): `UNIT_FLAG_NOT_SELECTABLE`
/// clear, a reaction of at least 4, and for a unit that is not player-controlled, `IsPvP 0x605ff0`
/// on its owner. The 4 is the internal scale [`ring_reaction`] returns (`UnitReaction 0x5167e0`
/// adds 1 for Lua at `0x51683e`), so neutral fails.
///
/// The player-controlled arm (`0x60673e`..`0x60679f`) reads an `[obj+0xe68]` record whose fields
/// are unnamed and is not built: a player-controlled unit passes, which never hides a buff.
pub(crate) fn can_assist(
    store: Option<&ObjectStore>,
    factions: Option<&Factions>,
    reputations: &Reputations,
    self_store: Option<&ObjectStore>,
    owner_store: impl FnOnce(u64) -> Option<ObjectStore>,
) -> bool {
    let Some(store) = store else {
        return false;
    };
    let flags = store.0.unit_flags();
    if flags & UNIT_FLAG_NOT_SELECTABLE != 0 {
        return false;
    }
    if ring_reaction(factions, reputations, Some(store), self_store) < 4 {
        return false;
    }
    if flags & UNIT_FLAG_PLAYER_CONTROLLED != 0 {
        // The player-controlled arm, not built: passes.
        return true;
    }
    // `IsPvP 0x605ff0`: the owner's flag (charmedBy, else createdBy, `0x5ee5a0`), else the unit's.
    let owned = store
        .0
        .unit_owner(OwnerFallback::CreatedBy)
        .and_then(owner_store);
    let pvp_flags = owned.as_ref().map_or(flags, |o| o.0.unit_flags());
    pvp_flags & UNIT_FLAG_PVP != 0
}
