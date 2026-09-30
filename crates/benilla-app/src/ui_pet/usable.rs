//! The pet-usability predicate `0x4bcf70`, asked once and by everything the reference lets it
//! gate: `GetPetActionsUsable` (`0x4be0b3`), the dispatcher behind `CastPetAction` and the orders
//! (`0x4bd1f2`), both autocast toggles (`0x4bcbcf`, `0x4bccce`) and the bar write the drag makes
//! (`0x4bc9d0`, for the drop `0x4bce33` and the pickup's blank `0x4be27f`). A zero answer leaves
//! each of them at its epilogue, before anything is latched, armed, cancelled or sent.

use benilla_protocol::messages::{OwnerFallback, PET_UNUSABLE_UNIT_FLAGS};

use crate::net::ObjectStore;

use super::{PetBar, PetUnit};

/// `0x4bcf70`, every exit in the order the bytes take them. `me` is the client's active player
/// guid (`0x468550`); `player` is that guid's object and `pet` the bar's cached guid's (typemasks
/// `0x10` and `8`), each `None` while it is not streamed.
pub(super) fn actions_usable(
    bar: &PetBar,
    me: Option<u64>,
    player: Option<&ObjectStore>,
    pet: Option<&ObjectStore>,
) -> bool {
    // `0x4bcf92`: the active player must resolve.
    let (Some(me), Some(player)) = (me, player) else {
        return false;
    };
    // `0x4bcf98`-`0x4bcfa4`: a charmed player, whose `UNIT_FIELD_CHARMEDBY` pair is nonzero, has
    // no pet bar to work.
    if player.0.unit_charmed_by().is_some() {
        return false;
    }
    // `0x4bcfbf`-`0x4bd012`: a view held on some unit, `PLAYER_FARSIGHT` (player field 712, at
    // `[[player+0xe68]+0x830]`), must be on the pet: Eyes of the Beast keeps the bar, Far Sight or
    // Mind Vision on anything else takes it. The two compares of the player's own guid against the
    // active guid (`0x4bcfb7`, `0x4bcfe0`) are the lookup's key against itself, so the branch that
    // zeroes the held guid (`0x4bcfe8`) has no way in.
    if player
        .0
        .player_farsight()
        .is_some_and(|held| held != bar.spells.pet_guid)
    {
        return false;
    }
    // `0x4bd034`-`0x4bd03d`: the cached guid must resolve to a unit.
    let Some(pet) = pet else {
        return false;
    };
    // `0x4bd03f`-`0x4bd067`: the pet's charmed-by guid, or its summoned-by guid while that is
    // zero, must be the active player's.
    if pet.0.unit_owner(OwnerFallback::SummonedBy) != Some(me) {
        return false;
    }
    // `0x4bd06f`-`0x4bd08b`: stunned (`0x40000`), confused (`0x400000`) and fleeing (`0x800000`).
    // Health is never read.
    if pet.0.unit_flags() & PET_UNUSABLE_UNIT_FLAGS != 0 {
        return false;
    }
    // `0x4bd08d`: bit 27 of the bar state `[0xb71468]`, which only the server writes.
    !bar.spells.bar_disabled()
}

impl PetUnit<'_, '_> {
    /// [`actions_usable`] for the bar as it stands: the pet's object when the predicate holds,
    /// `None` when it does not. A caller past the gate holds the pet the predicate resolved, which
    /// the dispatcher's own later lookups of the same guid find again (`0x4bd346`, `0x4bd403`).
    pub(crate) fn usable_pet(&self, bar: &PetBar) -> Option<&ObjectStore> {
        let pet = self.store(bar.spells.pet_guid);
        pet.filter(|_| actions_usable(bar, self.self_guid.0, self.player_store(), pet))
    }

    /// `GetPetActionsUsable()`, and the gate every pet-bar verb the reference guards asks.
    pub(crate) fn actions_usable(&self, bar: &PetBar) -> bool {
        self.usable_pet(bar).is_some()
    }
}
