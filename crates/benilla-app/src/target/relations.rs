//! **Who may I attack, who may I interact with, who may I help** — the three unit-relationship
//! predicates the reference shares across systems, kept together because they are one concern and
//! none belongs to the system that happens to call it first.
//!
//! The first two are byte-verified **complete** functions and live next door in [`super::ring`],
//! beside the two reaction directions they turn on — because *which direction* is the whole
//! content of both (`0x6061e0(this = player)` answers a reputation-slot faction with the **at-war
//! bit**; `0x6061e0(this = unit)` reads the standing, and the two routinely disagree). This file
//! is the shared store-only entry point every consumer calls:
//!
//! - `CanAttack 0x606980` → [`super::ring::can_attack_from_player`],
//! - `CanInteract 0x6067f0` → [`super::ring::can_interact_from_player`],
//! - `CanAssist 0x6066f0` → still the reaction-rank derivation below, on [`ring_reaction`].
//!
//! They lived in `scan.rs` while `can_attack` had exactly one caller; `can_assist` (the `UnitBuff`
//! gate, decision 1035) made that a second concern in a file about TAB targeting.

use benilla_protocol::messages::{ObjectType, OwnerFallback};

use crate::net::{ObjectStore, Reputations};

use super::{ring_reaction, Factions};

/// `OBJECT_FIELD_TYPE` bit 4 — the reference's own "is this a Player" test, read as
/// `[obj->descriptorBlock[2] + 8] >> 4 & 1` (`0x606984` in `CanAttack`, `0x6067fc` in
/// `CanInteract`). Taken off the object's own field rather than the spawning `NetEntity`, because
/// that is the bit the binary reads and it cannot drift from the store the rest of the predicate
/// walks.
///
/// **The one thing not verified against a live stream** is whether the server always puts field 2
/// in the create block (vmangos sends every non-zero field, and a player's is `0x19`, so it should
/// always be there — but that is a reading of the emulator, not an observation). The failure mode
/// is bounded and inert: this feeds only `CanAttack`'s ghost leg, so a false negative on a real
/// player skips a refusal that the both-player-controlled arm then makes anyway — a ghost carries
/// no duel, PvP flag or FFA pair. `/reaction` prints this beside the `NetEntity` kind so the first
/// hover over a player settles it by observation.
fn is_player_object(store: Option<&ObjectStore>) -> bool {
    store.and_then(|s| s.0.object_type()) == Some(ObjectType::Player)
}

/// `CanAttack 0x606980` — the shared attackability predicate: the world cursor's sword leg
/// (`0x48269a`), the combat flash's gate, the TAB scan's filter 3, `UnitCanAttack`, and hostile
/// spell targeting all ask this one question.
///
/// **This forwards to the complete function** ([`super::ring::can_attack_from_player`]) — a
/// ghost gate, five `UNIT_FIELD_FLAGS` refusal bits on the target, four cross-flag immunity legs,
/// then three terminal arms selected by `UNIT_FLAG_PVP_ATTACKABLE` on both parties.
///
/// It used to be `flag disqualifiers && ring_reaction ≤ 3`, and **the threshold was reading the
/// wrong reaction direction** — the very substitution decision 1530 named as the shipped defect
/// and corrected for nameplates alone. `0x606980`'s mixed arm is `UnitReaction(**player** → target)
/// < 4`, and that direction answers a reputation-slot faction with the **at-war bit**, never the
/// standing. Keeping the standing here made every not-at-war neutral faction attackable: hovering
/// a Cenarion Circle NPC drew the sword, TAB targeted it, and `UnitCanAttack` agreed (decision 1674).
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

/// `CanInteract 0x6067f0` — "may I take a service from this unit?", the gate the world-cursor
/// classifier runs at `0x482310` (through the `CanInteractNow 0x606880` wrapper) to choose between
/// the NPC service ladder and the loot/skin/attack block.
///
/// Forwards to [`super::ring::can_interact_from_player`], which is the complete `0x6067f0`.
pub(crate) fn can_interact(
    store: Option<&ObjectStore>,
    factions: Option<&Factions>,
    reputations: &Reputations,
    self_store: Option<&ObjectStore>,
) -> bool {
    super::ring::can_interact_from_player(factions, reputations, store, self_store)
}

/// `UNIT_FLAG_NOT_SELECTABLE` (bit 25) — `CanAssist`'s own first disqualifier, and one of
/// `CanAttack`'s five refusal bits (vmangos `UnitDefines.h`).
const UNIT_FLAG_NOT_SELECTABLE: u32 = 1 << 25;

/// `IsSelectable` — CGUnit_C's **vtable slot 21** (`0x60be60`), the predicate every selection path
/// reaches through the `+0x58` thunk `0x5f1ec0` (`mov eax,[ecx]; jmp [eax+0x54]`):
///
/// ```text
/// 60be60  mov  eax,[ecx+0x110]        ; the descriptor block
/// 60be6f  shr  ecx,0x19 / test cl,1   ; UNIT_FIELD_FLAGS bit 25 (0x02000000) — clear ⇒ return 1
/// 60be82  cmp  [eax+0x20],…           ; else UNIT_FIELD_CREATEDBY == the active player's guid
/// 60be94  mov  eax,1 / ret            ;      ⇒ return 1, otherwise 0
/// ```
///
/// So: **`NOT_SELECTABLE` clear, OR the unit is something *I* created.** The `CREATEDBY` clause is
/// the asymmetry worth carrying — your own flagged creations (your totems, your traps) stay
/// selectable, somebody else's do not.
///
/// Slot `+0x58` is the base stub `0x469fe0` (`xor eax,eax`) in **every** vtable but CGUnit_C's
/// (`0x80c4f8`) and CGPlayer_C's (`0x80af78`), so a GameObject, an item or a corpse answers
/// **false** here — which is the reason a non-unit guid can never become the selection.
///
/// **`None` is the reference's skipped vcall, not a refusal.** `SetSelection` only makes the call
/// on an object the manager actually resolved (`0x4935c6`/`0x4935c8` falls straight through to the
/// commit when it does not), which is how an out-of-range party member — on the roster, no
/// streamed object — stays selectable. A missing store here is that same state, so it passes; and
/// a store whose `OBJECT_FIELD_TYPE` has not streamed is read the same permissive way rather than
/// refusing a selection over an absent field.
///
/// `SetSelection 0x493540` makes this call at `0x4935ee` and returns on a false answer at
/// `0x4935f3`; three further consumers test the same bit (`0x6066f0`, `0x606829`, `0x60f600`).
pub(crate) fn is_selectable(store: Option<&ObjectStore>, self_guid: Option<u64>) -> bool {
    let Some(store) = store else {
        return true; // no resolved object — the reference never makes the call (`0x4935c8`)
    };
    // The base stub: only a CGUnit_C / CGPlayer_C carries a real slot-21 override.
    if !matches!(
        store.0.object_type(),
        None | Some(ObjectType::Unit) | Some(ObjectType::Player)
    ) {
        return false;
    }
    store.0.unit_flags() & UNIT_FLAG_NOT_SELECTABLE == 0
        || (self_guid.is_some() && store.0.unit_created_by() == self_guid)
}

/// `UNIT_FLAG_PVP` (bit 12) — what `IsPvP 0x605ff0` tests, after resolving the unit's owner
/// (vmangos `UnitDefines.h:UNIT_FLAG_PVP = 0x1000`).
const UNIT_FLAG_PVP: u32 = 0x1000;
/// `UNIT_FLAG_PVP_ATTACKABLE` (bit 3) — behaviourally "player-controlled", the same bit
/// [`ring_reaction`]'s duel leg selects on.
const UNIT_FLAG_PLAYER_CONTROLLED: u32 = 0x8;

/// `CanAssist 0x6066f0` — "may I help this unit?", the predicate `UnitBuff`'s unit-level gate runs
/// (see [`crate::ui_aura::buffs_visible_on`]). Named by its own Lua registrar pair
/// (`.data 0x8504c8 = {"UnitCanAssist", 0x516bb0}`), not by resemblance.
///
/// Three clauses: `UNIT_FLAG_NOT_SELECTABLE` clear, `UnitReaction ≥ 4`, and — for a unit that is
/// **not** player-controlled — `IsPvP 0x605ff0` on the unit (owner-chased). The reaction bar is the
/// **internal** scale, not the Lua one: `UnitReaction 0x5167e0` does `inc eax` at `0x51683e` before
/// pushing, so Lua's FRIENDLY(5) is internal 4. [`ring_reaction`] already returns the raw internal
/// rank, so the comparison is against 4 directly — **neutral fails**.
///
/// The player-controlled arm (`0x60673e`–`0x60679f`) keys on an `[obj+0xe68]` record whose fields
/// are **not** named, so it is deliberately NOT modelled: a player-controlled unit takes the
/// permissive answer here. That is conservative in the only direction that matters (it shows what
/// we already showed) and it keeps the un-derived arm out of the one place it could silently blank
/// a player's or a pet's buffs.
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
        // The un-derived arm — see the doc comment.
        return true;
    }
    // `IsPvP 0x605ff0`: the flag on the unit's OWNER when it has one (charmedBy, else createdBy —
    // the `0x5ee5a0` fallback pair), otherwise on the unit itself.
    let owned = store
        .0
        .unit_owner(OwnerFallback::CreatedBy)
        .and_then(owner_store);
    let pvp_flags = owned.as_ref().map_or(flags, |o| o.0.unit_flags());
    pvp_flags & UNIT_FLAG_PVP != 0
}
