//! The three unit-relationship predicates the reference shares across systems, may I attack,
//! interact with or help this unit, as the store-only entry points every consumer calls:
//!
//! - `CanAttack 0x606980` forwards to [`super::ring::can_attack_from_player`],
//! - `CanInteract 0x6067f0` forwards to [`super::ring::can_interact_from_player`],
//! - `CanAssist 0x6066f0` is derived here, on [`reaction_from_player`] or [`ring_reaction`].
//!
//! The first two live in [`super::ring`] beside the reaction directions they turn on:
//! `0x6061e0(this = player)` answers a reputation faction with the at-war bit,
//! `0x6061e0(this = unit)` with the standing, and the two often disagree.
//!
//! A corpse has its own reaction gate, `0x6067d0` ([`corpse_friendly`]).

use benilla_protocol::messages::{ObjectType, OwnerFallback};

use crate::net::{ObjectStore, Reputations};

use super::ring::reaction_from_player;
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

/// `UNIT_FLAG_POSSESSED`, `UNIT_FIELD_FLAGS` bit 24 (vmangos `UnitDefines.h:569`), which the
/// reference reads as `[[unit+0x110]+0xA3] & 1`.
pub(crate) const UNIT_FLAG_POSSESSED: u32 = 0x0100_0000;

/// Whether the active player possesses this unit (Mind Control, an Eye of Kilrogg, Eyes of the
/// Beast): `UNIT_FLAG_POSSESSED` set and `UNIT_FIELD_CHARMEDBY`, else `UNIT_FIELD_CREATEDBY`, the
/// active player's guid. The own-pet resolver `0x5ee5a0` (`0x5ee626`, `0x5ee62f`) and the world
/// pick's filter `0x480610` (`0x48064f`-`0x480674`) read this same clause. No store, no owner or
/// no active player is no possession.
pub(crate) fn is_possessed_by(store: Option<&ObjectStore>, self_guid: Option<u64>) -> bool {
    let (Some(store), Some(me)) = (store, self_guid) else {
        return false;
    };
    store.0.unit_flags() & UNIT_FLAG_POSSESSED != 0
        && store.0.unit_owner(OwnerFallback::CreatedBy) == Some(me)
}

/// `0x6067d0(player, corpse)`, the corpse's reaction gate: `0x6064c0` compares the player's
/// template (`UNIT_FIELD_FACTIONTEMPLATE`) toward the dead player's race's (`0x5d7120`: the race
/// byte of `CORPSE_FIELD_BYTES_1`, then `ChrRaces.dbc` column 2) with the template comparator
/// `0x606640` alone, no reputation, and passes at friendly (`setge` 4). A missing row on either
/// side is neutral (`0x60651b`), so it fails, as does a missing catalog or player.
pub(crate) fn corpse_friendly(
    corpse: &ObjectStore,
    factions: Option<&Factions>,
    self_store: Option<&ObjectStore>,
) -> bool {
    use benilla_formats::Reaction;
    let reaction = (|| {
        let catalog = factions?.catalog();
        let own = catalog.template(self_store?.0.unit_faction_template()?)?;
        Some(own.reaction_toward(catalog.race_template(corpse.0.corpse_race())?))
    })();
    reaction.unwrap_or(Reaction::Neutral) >= Reaction::Friendly
}

/// `UNIT_FLAG_PVP`, bit 12 (vmangos `UnitDefines.h:557`), what `IsPvP 0x605ff0` tests on the
/// unit's owner.
const UNIT_FLAG_PVP: u32 = 0x1000;
/// `UNIT_FIELD_FLAGS` bit 3, player-controlled (vmangos `UnitDefines.h:548`), the bit
/// [`ring_reaction`]'s duel leg selects on.
const UNIT_FLAG_PLAYER_CONTROLLED: u32 = 0x8;

/// Which of the two objects `CanAssist 0x6066f0` takes is the local player: its `this` (the
/// assister) or its argument (the one assisted).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Assist {
    /// `this` the player, the argument the unit: `UnitBuff` (`0x5195e0`), the TAB scan's friend
    /// mode (`0x493eca`), the cast binder (`0x6e45a2`) and `UnitCanAssist("player", unit)`
    /// (`0x516c05`).
    PlayerToUnit,
    /// `this` the unit, the argument the player: the spirit guide's acquisition (`0x4924c0`,
    /// `0x492506`).
    UnitToPlayer,
}

/// `CanAssist 0x6066f0` (Lua `UnitCanAssist` `0x516bb0`, registered at `.data 0x8504c8`), over the
/// unit and the local player in the roles [`Assist`] names. The argument's `UNIT_FLAG_NOT_SELECTABLE`
/// refuses first (`0x6066f4`..`0x606713`), then `this`' reaction toward the argument must be at
/// least 4 (`0x606719`..`0x606721`): `this` the player is [`reaction_from_player`] (the at-war bit
/// answers a reputation faction), `this` the unit [`ring_reaction`] (the standing). The 4 is the
/// internal scale (`UnitReaction 0x5167e0` adds 1 for Lua at `0x51683e`), so neutral fails.
///
/// Then the argument decides (`0x60672b`..`0x6067cd`): player-controlled, it takes the arm that
/// reads an `[obj+0xe68]` record whose fields are unnamed and is not built, so it passes. Otherwise
/// a player-controlled `this` needs `IsPvP 0x605ff0` of the argument, its owner's flag (charmedBy,
/// else createdBy, `0x5ee5a0`) else its own, and any other `this` passes.
pub(crate) fn can_assist(
    store: Option<&ObjectStore>,
    factions: Option<&Factions>,
    reputations: &Reputations,
    self_store: Option<&ObjectStore>,
    owner_store: impl FnOnce(u64) -> Option<ObjectStore>,
    roles: Assist,
) -> bool {
    let (Some(unit), Some(me)) = (store, self_store) else {
        return false;
    };
    let (this, arg) = match roles {
        Assist::PlayerToUnit => (me, unit),
        Assist::UnitToPlayer => (unit, me),
    };
    let arg_flags = arg.0.unit_flags();
    if arg_flags & UNIT_FLAG_NOT_SELECTABLE != 0 {
        return false;
    }
    let reaction = match roles {
        Assist::PlayerToUnit => reaction_from_player(factions, reputations, Some(unit), Some(me)),
        Assist::UnitToPlayer => ring_reaction(factions, reputations, Some(unit), Some(me)),
    };
    if reaction < 4 {
        return false;
    }
    if arg_flags & UNIT_FLAG_PLAYER_CONTROLLED != 0 {
        // The player-controlled arm, not built: passes.
        return true;
    }
    if this.0.unit_flags() & UNIT_FLAG_PLAYER_CONTROLLED == 0 {
        return true;
    }
    // `IsPvP 0x605ff0`: the owner's flag, else the argument's.
    let owned = arg
        .0
        .unit_owner(OwnerFallback::CreatedBy)
        .and_then(owner_store);
    let pvp_flags = owned.as_ref().map_or(arg_flags, |o| o.0.unit_flags());
    pvp_flags & UNIT_FLAG_PVP != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spell::targeting::corpse_fixture as fx;

    /// `UNIT_FIELD_FLAGS` bits: player-controlled, PvP, and not selectable.
    const PC: u32 = 0x8;
    const PVP: u32 = 0x1000;
    const NOT_SELECTABLE: u32 = 1 << 25;

    fn unit(template: u32, flags: u32) -> ObjectStore {
        // 35 = UNIT_FIELD_FACTIONTEMPLATE, 46 = UNIT_FIELD_FLAGS.
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (35, template),
            (46, flags),
        ]))
    }

    /// `this` the player asks `IsPvP` of the argument when the argument is not player-controlled
    /// (`0x6067a7`..`0x6067c2`), and a player-controlled argument takes the unbuilt arm and
    /// passes; with the roles swapped the argument is the player, who is. The argument's
    /// `UNIT_FLAG_NOT_SELECTABLE` refuses first, whoever it is.
    #[test]
    fn the_arguments_flags_and_the_assisters_control_decide_the_tail() {
        let factions = fx::factions();
        let reps = Reputations::default();
        let ask = |unit: &ObjectStore, me: &ObjectStore, roles| {
            can_assist(
                Some(unit),
                Some(&factions),
                &reps,
                Some(me),
                |_| None,
                roles,
            )
        };
        let me = unit(1, PC);
        // A friendly creature: only a PvP flag lets the player assist it.
        let plain = unit(1, 0);
        let flagged = unit(1, PVP);
        assert!(!ask(&plain, &me, Assist::PlayerToUnit));
        assert!(ask(&flagged, &me, Assist::PlayerToUnit));
        // Not selectable: refused.
        assert!(!ask(
            &unit(1, PVP | NOT_SELECTABLE),
            &me,
            Assist::PlayerToUnit
        ));
        // Swapped, the argument is the player: a player-controlled one passes with no PvP flag
        // on the unit, and its own NOT_SELECTABLE refuses.
        assert!(ask(&plain, &me, Assist::UnitToPlayer));
        assert!(!ask(
            &plain,
            &unit(1, PC | NOT_SELECTABLE),
            Assist::UnitToPlayer
        ));
        // A creature that is no friend (an orc to a human) fails the reaction either way.
        assert!(!ask(&unit(2, PVP), &me, Assist::PlayerToUnit));
        assert!(!ask(&unit(2, 0), &me, Assist::UnitToPlayer));
        // No player, no unit: nothing to ask.
        assert!(!can_assist(
            None,
            Some(&factions),
            &reps,
            Some(&me),
            |_| None,
            Assist::PlayerToUnit
        ));
        assert!(!can_assist(
            Some(&me),
            Some(&factions),
            &reps,
            None,
            |_| None,
            Assist::PlayerToUnit
        ));
    }

    /// The reaction is the assister's toward the argument: with the player as `this` a reputation
    /// faction answers by the at-war bit (`0x6063e0`..`0x606426`), not the standing. Cenarion
    /// Circle (faction 609, slot 36) at a neutral standing is therefore assistable by the player
    /// until war is declared on it, though its standing reads neutral (3 < 4) toward the player.
    #[test]
    fn the_player_assists_a_not_at_war_reputation_faction_whose_standing_is_neutral() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let factions =
            Factions::from_catalog(benilla_formats::load_faction_catalog(&mut chain).expect("dbc"));
        let me = unit(1, PC);
        let circle = unit(996, PVP);
        let quiet = Reputations::default();
        let mut slots = vec![(0u8, 0i32); 64];
        slots[36] = (benilla_formats::faction_flags::AT_WAR, 0);
        let at_war = Reputations::from_slots(slots);
        let ask = |reps: &Reputations, roles| {
            can_assist(
                Some(&circle),
                Some(&factions),
                reps,
                Some(&me),
                |_| None,
                roles,
            )
        };
        assert!(
            ask(&quiet, Assist::PlayerToUnit),
            "player toward unit: not at war"
        );
        assert!(!ask(&at_war, Assist::PlayerToUnit), "at war: hostile");
        // The unit toward the player is the standing, neutral either way: the spirit guide's
        // direction.
        assert!(!ask(&quiet, Assist::UnitToPlayer));
        assert!(!ask(&at_war, Assist::UnitToPlayer));
    }

    /// `0x6067d0`: the caster's template toward the corpse's race's, friendly or better; every gap
    /// is neutral (`0x60651b`) and fails.
    #[test]
    fn the_corpse_gate_reads_the_race_template() {
        let factions = fx::factions();
        let me = fx::caster(1.5);
        let gate = |corpse: &ObjectStore| corpse_friendly(corpse, Some(&factions), Some(&me));
        assert!(
            gate(&fx::corpse(fx::HUMAN, false)),
            "a human toward a human"
        );
        assert!(!gate(&fx::corpse(fx::ORC, false)), "a human toward an orc");
        assert!(!gate(&fx::corpse(0, false)), "race 0 has no ChrRaces row");
        assert!(
            !gate(&ObjectStore(Default::default())),
            "no BYTES_1 reads race 0"
        );
        let human = fx::corpse(fx::HUMAN, false);
        assert!(!corpse_friendly(&human, None, Some(&me)), "no catalog");
        assert!(!corpse_friendly(&human, Some(&factions), None), "no player");
        let no_template = ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(35, 999)]));
        assert!(
            !corpse_friendly(&human, Some(&factions), Some(&no_template)),
            "a template the table lacks"
        );
    }

    /// The same gate on the shipped `ChrRaces.dbc` and `FactionTemplate.dbc`: a human (template 1)
    /// stands friendly toward every Alliance race's corpse and not toward a Horde one's.
    #[test]
    fn the_corpse_gate_splits_the_sides_on_the_shipped_tables() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let factions = Factions::from_catalog(
            benilla_formats::load_faction_catalog(&mut chain).expect("faction tables"),
        );
        let me = fx::caster(1.5);
        let gate = |race| corpse_friendly(&fx::corpse(race, false), Some(&factions), Some(&me));
        for race in [1, 3, 4, 7] {
            assert!(gate(race), "Alliance race {race}");
        }
        for race in [2, 5, 6, 8] {
            assert!(!gate(race), "Horde race {race}");
        }
        assert!(!gate(0) && !gate(10), "a race with no row");
    }

    /// `0x480658`-`0x480667`: the flag, then the charmer if there is one, else the creator; an
    /// unset owner or an unknown active player is no possession.
    #[test]
    fn possession_is_the_flag_and_the_charmer_else_the_creator() {
        let store = |flags: u32, charmed_by: u32, created_by: u32| {
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (46, flags),
                (10, charmed_by),
                (11, 0),
                (14, created_by),
                (15, 0),
            ]))
        };
        let me = Some(0x77);
        let flag = UNIT_FLAG_POSSESSED;
        assert!(
            is_possessed_by(Some(&store(flag, 0x77, 0)), me),
            "charmed by us"
        );
        assert!(
            is_possessed_by(Some(&store(flag, 0, 0x77)), me),
            "created by us"
        );
        assert!(
            !is_possessed_by(Some(&store(flag, 0x99, 0x77)), me),
            "a charmer other than us comes before our creation"
        );
        assert!(!is_possessed_by(Some(&store(0, 0x77, 0x77)), me), "no flag");
        assert!(!is_possessed_by(Some(&store(flag, 0, 0)), me), "no owner");
        assert!(
            !is_possessed_by(Some(&store(flag, 0, 0)), None),
            "no active player"
        );
        assert!(!is_possessed_by(None, me), "no descriptor");
    }
}
