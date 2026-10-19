//! The reference's relations between two units, the one model every system and the Lua API ask:
//! `UnitReaction 0x6061e0`, `CanAttack 0x606980`, `CanAssist 0x6066f0`, `IsPvP 0x605ff0`,
//! `CanCooperate 0x606ba0` and `CanInteract 0x6067f0`, each with `this` first and its argument
//! second, over two [`RelUnit`]s and the state in [`Relations`]: the faction tables, the
//! reputation list, the active player, the party slots and the object manager's unit lookup.
//!
//! None of them is symmetric. Each maps a player-controlled unit to the player controlling it
//! ([`Relations::controller`], `0x606170`) before its player-versus-player rungs, so a pet or a
//! charmed unit answers with its controller's duel, party, free-for-all and PvP state, and the
//! reputation legs apply only to a unit the active player controls.
//!
//! A corpse has its own reaction gate, `0x6067d0` ([`corpse_friendly`]).

use std::marker::PhantomData;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_formats::{reputation_rank, Reaction};
use benilla_protocol::messages::{ObjectType, OwnerFallback};

use super::Factions;
use crate::names::NameCache;
use crate::net::{Guid, GuidIndex, ObjectStore, Reputations, SelfGuid, SelfPlayer};
use crate::spell::group_relation::GroupRoster;

/// `UnitReaction`'s three anchors on the internal scale; Lua adds 1 (`0x51683e`).
const HOSTILE: u8 = Reaction::Hostile as u8;
const NEUTRAL: u8 = Reaction::Neutral as u8;
const FRIENDLY: u8 = Reaction::Friendly as u8;
/// Revered, the highest rank the template leg answers (`0x606439`).
const REACTION_CAP: u8 = 6;

/// `UNIT_FIELD_FLAGS` (vmangos `UnitDefines.h:490-514`). Player-controlled (bit 3) is what every
/// player-versus-player rung needs on both sides (`0x606217`, `0x60622f`).
const UNIT_FLAG_PLAYER_CONTROLLED: u32 = 0x8;
/// Bits 8 and 9, which `CanAttack` reads across the controlled line both ways (`0x606a05`-
/// `0x606a8a`), and bit 8 also `IsPvP` on an owned unit (`0x60602c`).
const UNIT_FLAG_IMMUNE_TO_PLAYER: u32 = 0x100;
const UNIT_FLAG_IMMUNE_TO_NPC: u32 = 0x200;
/// Bit 12, `IsPvP`'s answer (`0x60603e`) and `CanAttack`'s on the victim's player (`0x606b45`).
const UNIT_FLAG_PVP: u32 = 0x1000;
/// Bit 14, which raises a reputation faction's reaction toward this unit a rank (`0x606464`);
/// vmangos `UNIT_FLAG_UNK_14`.
const UNIT_FLAG_RAISES_REACTION: u32 = 0x4000;
/// Bit 25, `CanAssist`'s first refusal (`0x606706`) and one of `CanAttack`'s.
const UNIT_FLAG_NOT_SELECTABLE: u32 = 1 << 25;
/// The victim bits any one of which makes `CanAttack` refuse (`0x6069b7`-`0x6069ff`): 1, 7, 16,
/// 20 (taxi) and 25.
const CANNOT_BE_ATTACKED: u32 = 0x2 | 0x80 | 0x1_0000 | 0x10_0000 | UNIT_FLAG_NOT_SELECTABLE;
/// `PLAYER_FLAGS` (vmangos `Player.h:319-323`): ghost, free-for-all and contested.
const PLAYER_FLAGS_GHOST: u32 = 0x10;
const PLAYER_FLAGS_FFA_PVP: u32 = 0x80;
const PLAYER_FLAGS_CONTESTED_PVP: u32 = 0x100;
/// The template flag that turns its units on a contested player (`0x6065be`).
const FACTION_TEMPLATE_FLAG_ATTACK_CONTESTED: u32 = 0x1000;
/// The creature record's `type_flags` bit `0x2`, VISIBLE_TO_GHOSTS (`0x605f70`).
const TYPE_FLAG_VISIBLE_TO_GHOSTS: u32 = 0x2;

/// One unit as the relation functions read it: its guid and the descriptor fields they test,
/// copied off its [`ObjectStore`] ([`RelUnit::of`]). A field the store has not streamed reads 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(Default))]
pub(crate) struct RelUnit {
    /// `OBJECT_FIELD_GUID`, which the identity, party and persuaded tests compare. Never 0: the
    /// reference's units are live objects, so the persuaded test (`0x606449`) compares a field
    /// that reads 0 for none with a guid that never is, and a unit whose guid is not yet known is
    /// not built.
    pub(crate) guid: u64,
    /// `OBJECT_FIELD_TYPE` carries the player bit `0x10` (`0x6061c6`, `0x606987`).
    pub(crate) is_player: bool,
    /// `UNIT_FIELD_FLAGS` (field 46, `[+0x110]+0xa0`).
    pub(crate) flags: u32,
    /// `UNIT_FIELD_FACTIONTEMPLATE` (field 35, `[+0x110]+0x74`).
    pub(crate) template: Option<u32>,
    /// `UNIT_FIELD_CHARMEDBY` (field 10), 0 for none.
    pub(crate) charmed_by: u64,
    /// `UNIT_FIELD_CREATEDBY` (field 14), 0 for none.
    pub(crate) created_by: u64,
    /// `UNIT_FIELD_PERSUADED` (field 18), 0 for none.
    pub(crate) persuaded: u64,
    /// `PLAYER_FLAGS` (`[+0xe68]+0x8`), 0 on a creature.
    pub(crate) player_flags: u32,
    /// `PLAYER_DUEL_ARBITER` (`[+0xe68]+0x0`) and `PLAYER_DUEL_TEAM` (`[+0xe68]+0x20`); the team
    /// is 0 until a duel's countdown ends.
    pub(crate) duel_arbiter: u64,
    pub(crate) duel_team: u32,
    /// `UNIT_NPC_FLAGS` (field 147), which `CanInteract` needs nonzero (`0x60683d`).
    pub(crate) npc_flags: u32,
    /// `UNIT_FIELD_BYTES_0`'s race and class. The active player's select the base its reputation
    /// standing is added to (`0x4d6370`).
    pub(crate) race: u8,
    pub(crate) class: u8,
    /// The creature record's VISIBLE_TO_GHOSTS bit (`0x605f70`): the only unit that can attack a
    /// ghost player, and the only one a ghost player can interact with. False for a player, and
    /// until the record arrives ([`RelUnit::with_record`]).
    pub(crate) ghost_visible: bool,
}

impl RelUnit {
    /// The unit `guid` names, read off its descriptor.
    pub(crate) fn of(guid: u64, store: &ObjectStore) -> Self {
        let f = &store.0;
        Self {
            guid,
            is_player: f.object_type() == Some(ObjectType::Player),
            flags: f.unit_flags(),
            template: f.unit_faction_template(),
            charmed_by: f.unit_charmed_by().unwrap_or(0),
            created_by: f.unit_created_by().unwrap_or(0),
            persuaded: f.unit_persuaded().unwrap_or(0),
            player_flags: f.player_flags(),
            duel_arbiter: f.player_duel_arbiter(),
            duel_team: f.player_duel_team(),
            npc_flags: f.unit_npc_flags(),
            race: f.unit_race().unwrap_or(0),
            class: f.unit_class().unwrap_or(0),
            ghost_visible: false,
        }
    }

    /// With the creature record's VISIBLE_TO_GHOSTS bit, `[[unit+0xb30]+0x14] & 0x2`: the template
    /// the name cache holds for the descriptor's `OBJECT_FIELD_ENTRY`. A player has no record.
    pub(crate) fn with_record(mut self, store: &ObjectStore, names: &NameCache) -> Self {
        self.ghost_visible = !self.is_player
            && store
                .0
                .object_entry()
                .and_then(|entry| names.creature_record(entry))
                .is_some_and(|r| r.type_flags & TYPE_FLAG_VISIBLE_TO_GHOSTS != 0);
        self
    }

    /// `UNIT_FIELD_FLAGS` bit 3: players, their pets and charmed units carry it.
    pub(crate) fn player_controlled(&self) -> bool {
        self.flags & UNIT_FLAG_PLAYER_CONTROLLED != 0
    }

    /// The reference compares object pointers (`0x606200`, `0x606ba6`); one object has one guid.
    fn same_object(&self, other: &Self) -> bool {
        self.guid == other.guid
    }

    /// The owner `0x606170` and `0x605ff0` look up: the charmer, else the creator, 0 for neither.
    fn owner(&self) -> u64 {
        if self.charmed_by != 0 {
            self.charmed_by
        } else {
            self.created_by
        }
    }
}

/// The object manager's lookup by guid with the unit typemask (`0x468460`, `ecx = 8`): `None` for
/// a guid it does not hold, or holds as anything but a unit.
pub(crate) trait UnitLookup {
    fn unit(&self, guid: u64) -> Option<RelUnit>;
}

/// The lookup over any reader of a guid's descriptor, the unit typemask applied: a caller's own
/// index and store query.
pub(crate) struct StoreLookup<'a, F>(F, PhantomData<&'a ObjectStore>);

impl<'a, F: Fn(u64) -> Option<&'a ObjectStore>> StoreLookup<'a, F> {
    pub(crate) fn new(store_of: F) -> Self {
        Self(store_of, PhantomData)
    }
}

impl<'a, F: Fn(u64) -> Option<&'a ObjectStore>> UnitLookup for StoreLookup<'a, F> {
    fn unit(&self, guid: u64) -> Option<RelUnit> {
        let store = (self.0)(guid).filter(|s| s.is_unit())?;
        Some(RelUnit::of(guid, store))
    }
}

/// Units held by guid: an owned copy of the object manager's.
impl UnitLookup for std::collections::HashMap<u64, RelUnit> {
    fn unit(&self, guid: u64) -> Option<RelUnit> {
        self.get(&guid).copied()
    }
}

/// No unit held: a caller with only the two units in hand, whose owners it cannot reach.
#[cfg(test)]
pub(crate) struct NoUnits;

#[cfg(test)]
impl UnitLookup for NoUnits {
    fn unit(&self, _: u64) -> Option<RelUnit> {
        None
    }
}

/// What the relation functions read beside their two units.
#[derive(Clone, Copy)]
pub(crate) struct Relations<'a> {
    /// The faction tables; without them every faction leg reads neutral.
    pub(crate) factions: Option<&'a Factions>,
    /// The active player's reputation list and the server's forced reactions.
    pub(crate) reputations: &'a Reputations,
    /// The active player's guid (`0x468550`); `None` before its object.
    pub(crate) me: Option<u64>,
    /// The party slots `0x4e7f70` reads; `None` reads as no party.
    pub(crate) roster: Option<&'a GroupRoster>,
    /// The object manager's unit lookup, which the controller and owner legs ask.
    pub(crate) units: &'a dyn UnitLookup,
}

impl Relations<'_> {
    /// `0x606170`: the player a unit answers to, its charmer else its creator (`0x60617d`), or the
    /// unit itself when neither is set (`0x6061bf`), and only when that object is a player
    /// (`0x6061c6`). An owner the object manager does not hold answers none.
    pub(crate) fn controller(&self, unit: &RelUnit) -> Option<RelUnit> {
        let owner = unit.owner();
        let object = if owner == 0 {
            *unit
        } else {
            self.units.unit(owner)?
        };
        object.is_player.then_some(object)
    }

    /// `0x4e7f70` (through `0x4918d0`): a nonzero guid that is the active player's or one of the
    /// four party slots'.
    pub(crate) fn in_party(&self, guid: u64) -> bool {
        match self.roster {
            Some(roster) => roster.in_party(self.me, guid),
            None => guid != 0 && self.me == Some(guid),
        }
    }

    /// Whether a guid is the active player's (`0x468550`).
    fn is_me(&self, guid: u64) -> bool {
        self.me.is_some_and(|me| me == guid)
    }

    /// `UnitReaction 0x6061e0(this = a, b)`, `a`'s reaction toward `b` on the internal scale, 0
    /// hated to 7 exalted. One object toward itself is friendly (`0x606200`). Two player-controlled
    /// units answer by their controlling players first ([`Self::player_rungs`]). Then a unit the
    /// active player controls answers by the player's reputation with `b`'s faction
    /// ([`Self::reputation_leg`]); anything else by `a`'s template toward `b`
    /// ([`Self::template_reaction`]), capped at 6 (`0x606439`) and raised a rank when `a` is
    /// persuaded by `b` (`0x606449`-`0x6064aa`).
    pub(crate) fn reaction(&self, a: &RelUnit, b: &RelUnit) -> u8 {
        if a.same_object(b) {
            return FRIENDLY;
        }
        if a.player_controlled() && b.player_controlled() {
            if let Some(rank) = self.player_rungs(a, b) {
                return rank;
            }
        }
        if let Some(rank) = self.reputation_leg(a, b) {
            return rank;
        }
        let rank = self.template_reaction(a.template, b).min(REACTION_CAP);
        // `0x606449`-`0x6064aa`: persuaded by `b`, or `b` carrying bit 14, raises 2, 3 and 4 a
        // rank, for a template whose faction has a reputation slot only.
        let persuaded = a.persuaded == b.guid || b.flags & UNIT_FLAG_RAISES_REACTION != 0;
        if persuaded && (2..5).contains(&rank) && self.has_reputation_slot(a.template) {
            return rank + 1;
        }
        rank
    }

    /// `0x606247`-`0x60636f`, the rungs between two player-controlled units, each on its
    /// controlling player: neutral when either has none (`0x60635f`), then the duel, the party and
    /// the both-free-for-all rungs. `None` falls through to the faction legs.
    fn player_rungs(&self, a: &RelUnit, b: &RelUnit) -> Option<u8> {
        let (Some(pa), Some(pb)) = (self.controller(a), self.controller(b)) else {
            return Some(NEUTRAL);
        };
        if let DuelRung::Engaged { rank, .. } = duel_rung(&pa, &pb) {
            return Some(rank);
        }
        // `0x6062b0`-`0x60631c`: the active player toward a party member, either way round. The
        // party test counts the active player too, so its own pet is friendly to it.
        if self.is_me(pa.guid) && self.in_party(pb.guid)
            || self.is_me(pb.guid) && self.in_party(pa.guid)
        {
            return Some(FRIENDLY);
        }
        // `0x60632c`-`0x60634f`: both players free-for-all.
        (pa.player_flags & pb.player_flags & PLAYER_FLAGS_FFA_PVP != 0).then_some(HOSTILE)
    }

    /// `0x606372`-`0x606423`: when the active player controls `a`, `b`'s faction answers by the
    /// player's reputation, a forced reaction first (`0x6063e0`, returned uncapped), else a
    /// faction with a reputation slot by its at-war bit alone (`0x60640c`: 1 at war, 4 not),
    /// whatever the standing. `None` for any other `a`, or a `b` template or faction with neither.
    fn reputation_leg(&self, a: &RelUnit, b: &RelUnit) -> Option<u8> {
        if !a.player_controlled() || !self.is_me(self.controller(a)?.guid) {
            return None;
        }
        let catalog = self.factions?.catalog();
        let row = catalog.template(b.template?)?;
        if let Some(rank) = forced_rank(self.reputations, row.faction) {
            return Some(rank);
        }
        at_war_with(catalog, self.reputations, row.faction).map(|at_war| {
            if at_war {
                HOSTILE
            } else {
                FRIENDLY
            }
        })
    }

    /// `0x606530(template, b)`, a faction template's reaction toward `b`: neutral when either row
    /// is missing (`0x606627`). When the active player controls `b` (`0x606588`-`0x6065b6`), a
    /// template flagged `0x1000` is hostile to a contested player (`0x6065be`, `0x6065d2`), then
    /// a forced reaction answers (`0x6065e6`), then a faction with a reputation slot the player's
    /// standing rank (`0x606609`); otherwise the template comparator `0x606640`.
    pub(crate) fn template_reaction(&self, template: Option<u32>, b: &RelUnit) -> u8 {
        let Some(catalog) = self.factions.map(Factions::catalog) else {
            return NEUTRAL;
        };
        let (Some(row_a), Some(row_b)) = (
            template.and_then(|t| catalog.template(t)),
            b.template.and_then(|t| catalog.template(t)),
        ) else {
            return NEUTRAL;
        };
        let mine = b
            .player_controlled()
            .then(|| self.controller(b))
            .flatten()
            .filter(|p| self.is_me(p.guid));
        if let Some(player) = mine {
            if row_a.flags & FACTION_TEMPLATE_FLAG_ATTACK_CONTESTED != 0
                && player.player_flags & PLAYER_FLAGS_CONTESTED_PVP != 0
            {
                return HOSTILE;
            }
            if let Some(rank) = forced_rank(self.reputations, row_a.faction) {
                return rank;
            }
            if let Some(info) = catalog.reputation_faction(row_a.faction) {
                // `0x4d6370`: the slot's base plus its standing, the base the active player's.
                let (race, class) = self
                    .me
                    .and_then(|g| self.units.unit(g))
                    .map_or((0, 0), |me| (me.race, me.class));
                let standing = self
                    .reputations
                    .slots
                    .get(info.rep_index as usize)
                    .map_or(0, |&(_flags, s)| s);
                return reputation_rank(info.base_for(race, class) + standing);
            }
        }
        row_a.reaction_toward(row_b) as u8
    }

    /// `0x605fc0` of a template's faction: its `Faction.dbc` row has a reputation slot.
    fn has_reputation_slot(&self, template: Option<u32>) -> bool {
        (|| {
            let catalog = self.factions?.catalog();
            let row = catalog.template(template?)?;
            catalog.reputation_faction(row.faction)
        })()
        .is_some()
    }

    /// `CanAttack 0x606980(this = a, b)`, may `a` attack `b`. A ghost player is attackable only by
    /// a creature visible to ghosts (`0x606987`-`0x6069b1`); five victim bits refuse
    /// (`0x6069b7`-`0x6069ff`); bits 8 and 9 refuse across the controlled line both ways
    /// (`0x606a05`-`0x606a8a`). Then two uncontrolled units fight when either reaction is hostile
    /// (`0x606aab`, `0x606ab8`), a mixed pair when `a`'s is below friendly (`0x606b7f`), and two
    /// controlled units, not at friendly (`0x606b03`), when either controlling player is missing,
    /// both are in one duel, `b`'s player is PvP-flagged, or both are free-for-all
    /// (`0x606b05`-`0x606b72`). A reaction is not this: an unflagged enemy player is hostile and
    /// not attackable, a same-faction duel opponent friendly and attackable.
    pub(crate) fn can_attack(&self, a: &RelUnit, b: &RelUnit) -> bool {
        if b.is_player && b.player_flags & PLAYER_FLAGS_GHOST != 0 && !a.ghost_visible {
            return false;
        }
        if b.flags & CANNOT_BE_ATTACKED != 0 {
            return false;
        }
        let (a_controlled, b_controlled) = (a.player_controlled(), b.player_controlled());
        if a_controlled && b.flags & UNIT_FLAG_IMMUNE_TO_PLAYER != 0
            || !a_controlled && b.flags & UNIT_FLAG_IMMUNE_TO_NPC != 0
            || a.flags & UNIT_FLAG_IMMUNE_TO_PLAYER != 0 && b_controlled
            || a.flags & UNIT_FLAG_IMMUNE_TO_NPC != 0 && !b_controlled
        {
            return false;
        }
        match (a_controlled, b_controlled) {
            (false, false) => self.reaction(a, b) <= HOSTILE || self.reaction(b, a) <= HOSTILE,
            (true, true) => {
                if self.reaction(a, b) >= FRIENDLY {
                    return false;
                }
                let (Some(pa), Some(pb)) = (self.controller(a), self.controller(b)) else {
                    return true;
                };
                // `0x606b1d`-`0x606b43`: one duel, teams set; the friendly refusal above already
                // took a duel partner on the same team.
                pa.duel_team != 0 && pb.duel_team != 0 && pa.duel_arbiter == pb.duel_arbiter
                    || pb.flags & UNIT_FLAG_PVP != 0
                    || pa.player_flags & pb.player_flags & PLAYER_FLAGS_FFA_PVP != 0
            }
            _ => self.reaction(a, b) < FRIENDLY,
        }
    }

    /// `CanAssist 0x6066f0(this = a, b)`, may `a` help `b`: `b` selectable (`0x606706`) and `a`'s
    /// reaction toward it friendly (`0x60671e`). A player-controlled `b` then needs, on the two
    /// controlling players (`0x60673e`), `a`'s in `b`'s duel on `b`'s team when `b`'s is
    /// duelling (`0x606756`-`0x60677a`), and `a`'s free-for-all when `b`'s is (`0x606784`-
    /// `0x60679d`); a missing player passes (`0x6067c8`). Any other `b` passes for an
    /// uncontrolled `a` and takes [`Self::is_pvp`] for a controlled one (`0x6067a7`-`0x6067bd`).
    pub(crate) fn can_assist(&self, a: &RelUnit, b: &RelUnit) -> bool {
        if b.flags & UNIT_FLAG_NOT_SELECTABLE != 0 {
            return false;
        }
        if self.reaction(a, b) < FRIENDLY {
            return false;
        }
        if b.player_controlled() {
            let (Some(pa), Some(pb)) = (self.controller(a), self.controller(b)) else {
                return true;
            };
            if pb.duel_team != 0
                && (pa.duel_arbiter != pb.duel_arbiter || pa.duel_team != pb.duel_team)
            {
                return false;
            }
            return pb.player_flags & PLAYER_FLAGS_FFA_PVP == 0
                || pa.player_flags & PLAYER_FLAGS_FFA_PVP != 0;
        }
        !a.player_controlled() || self.is_pvp(b)
    }

    /// `IsPvP 0x605ff0`: a unit with a charmer or creator the object manager holds as a unit
    /// (`0x606022`) answers no when it carries bit 8 (`0x60602c`), else with that owner's PvP
    /// flag (`0x60603e`); any other unit with its own (`0x606051`).
    pub(crate) fn is_pvp(&self, unit: &RelUnit) -> bool {
        let owner = unit.owner();
        match (owner != 0).then(|| self.units.unit(owner)).flatten() {
            Some(owner) => {
                unit.flags & UNIT_FLAG_IMMUNE_TO_PLAYER == 0 && owner.flags & UNIT_FLAG_PVP != 0
            }
            None => unit.flags & UNIT_FLAG_PVP != 0,
        }
    }

    /// `CanCooperate 0x606ba0(this = a, b)`: not one object (`0x606ba6`), neither charmed
    /// (`0x606bab`-`0x606bc5`), and both templates resolving to rows with one faction-group mask
    /// (`0x606bc7`-`0x606c07`). It reads no reaction, party or PvP state.
    pub(crate) fn can_cooperate(&self, a: &RelUnit, b: &RelUnit) -> bool {
        if a.same_object(b) || a.charmed_by != 0 || b.charmed_by != 0 {
            return false;
        }
        (|| {
            let catalog = self.factions?.catalog();
            let row_a = catalog.template(a.template?)?;
            let row_b = catalog.template(b.template?)?;
            Some(row_a.group_mask == row_b.group_mask)
        })()
        .unwrap_or(false)
    }

    /// `CanInteract 0x6067f0(this = a, b)`, may `a` take a service from `b`: a ghost player `a`
    /// only from a creature visible to ghosts (`0x606808`-`0x606821`), then `b` selectable
    /// (`0x606835`), with some `UNIT_NPC_FLAGS` (`0x60683d`), and both reactions at least neutral,
    /// `b`'s toward `a` (`0x606847`) and `a`'s toward `b` (`0x606854`).
    pub(crate) fn can_interact(&self, a: &RelUnit, b: &RelUnit) -> bool {
        if a.is_player && a.player_flags & PLAYER_FLAGS_GHOST != 0 && !b.ghost_visible {
            return false;
        }
        b.flags & UNIT_FLAG_NOT_SELECTABLE == 0
            && b.npc_flags != 0
            && self.both_at_least_neutral(a, b)
    }

    /// Both reactions between two units neutral or better, `b`'s toward `a` and `a`'s toward `b`:
    /// `CanInteract`'s test (`0x606847`, `0x606854`) and the select greeting's (`0x60c2d9`-
    /// `0x60c2f2`).
    pub(crate) fn both_at_least_neutral(&self, a: &RelUnit, b: &RelUnit) -> bool {
        self.reaction(b, a) >= NEUTRAL && self.reaction(a, b) >= NEUTRAL
    }
}

/// The forced reaction the server set for a `Faction.dbc` id (`SMSG_SET_FORCED_REACTIONS`, a
/// `SPELL_AURA_FORCE_REACTION` aura such as *King of the Gordok*'s); `None` when the faction is not
/// forced. The lookup `0x4d6490` returns the stored rank verbatim (`0x4d64b3`).
///
/// Deviation: the rank is held to 7, the top of the ring scale's eight entries (`0..=7`), where the
/// reference keeps any value; a server rank past 7 would index outside our palette.
pub(crate) fn forced_rank(reputations: &Reputations, faction: u32) -> Option<u8> {
    reputations
        .forced
        .iter()
        .find(|&&(id, _)| id == faction)
        .map(|&(_, rank)| rank.min(7) as u8)
}

/// Whether we are at war with the `Faction.dbc` faction a template names (`0x4d61b0`, bit 1 of its
/// reputation slot's flag byte); `None` when it has no reputation slot (`0x605fc0`). The bit is the
/// one the reputation pane's war checkbox writes, so declaring war moves the plate, the cursor and
/// attackability at once. `/reaction` prints it.
pub(crate) fn at_war_with(
    catalog: &benilla_formats::FactionCatalog,
    reputations: &Reputations,
    faction_id: u32,
) -> Option<bool> {
    let info = catalog.reputation_faction(faction_id)?;
    Some(
        usize::try_from(info.rep_index)
            .ok()
            .and_then(|i| reputations.slots.get(i))
            .is_some_and(|&(flags, _)| flags & benilla_formats::faction_flags::AT_WAR != 0),
    )
}

/// A unit's `FactionTemplate` faction-group mask (`row + 0xc`), the one number `CanCooperate`
/// compares: 3 on every Alliance race row, 5 on every Horde one. `/reaction` prints it.
pub(crate) fn faction_group_mask(factions: Option<&Factions>, unit: &RelUnit) -> Option<u32> {
    Some(factions?.catalog().template(unit.template?)?.group_mask)
}

/// Why the duel rung of `UnitReaction` (`0x60626b`-`0x6062ad`) did or did not fire between two
/// controlling players, with the values it judged; `/reaction` prints it from this same walk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DuelRung {
    /// `PLAYER_DUEL_TEAM` is 0 on a side: no duel, or its countdown has not ended.
    NoTeam { a: u32, b: u32 },
    /// Both have a team but different arbiters: two duels.
    ArbiterMismatch { a: u64, b: u64 },
    /// The rung fires: 4 (friendly) on one team (`0x606296`), 1 (hostile) on opposing teams.
    Engaged { rank: u8, a_team: u32, b_team: u32 },
}

/// The duel rung's walk on two controlling players: both teams nonzero (`0x606274`, `0x606284`)
/// and the arbiter guids equal (`0x606288`-`0x606294`). `PLAYER_DUEL_ARBITER` is set at the
/// challenge, `PLAYER_DUEL_TEAM` only when the countdown ends (`Player::UpdateDuelFlag`), so the
/// rung waits for the count.
pub(crate) fn duel_rung(a: &RelUnit, b: &RelUnit) -> DuelRung {
    if a.duel_team == 0 || b.duel_team == 0 {
        return DuelRung::NoTeam {
            a: a.duel_team,
            b: b.duel_team,
        };
    }
    if a.duel_arbiter != b.duel_arbiter {
        return DuelRung::ArbiterMismatch {
            a: a.duel_arbiter,
            b: b.duel_arbiter,
        };
    }
    DuelRung::Engaged {
        rank: if a.duel_team == b.duel_team {
            FRIENDLY
        } else {
            HOSTILE
        },
        a_team: a.duel_team,
        b_team: b.duel_team,
    }
}

/// The relation inputs as a system reads them: the faction tables, the reputation list, the active
/// player, the published party slots, the name cache's creature records and the object manager.
/// [`Self::get`] is the [`Relations`] over them.
#[derive(SystemParam)]
pub(crate) struct RelationWorld<'w, 's> {
    factions: Option<Res<'w, Factions>>,
    reputations: Res<'w, Reputations>,
    /// `Option`, as a UI-only harness has no net stack.
    self_guid: Option<Res<'w, SelfGuid>>,
    roster: Option<Res<'w, GroupRoster>>,
    names: Option<Res<'w, NameCache>>,
    index: Option<Res<'w, GuidIndex>>,
    stores: Query<'w, 's, &'static ObjectStore>,
    /// The active player's object, its guid and descriptor.
    self_q: Query<'w, 's, (Option<&'static Guid>, &'static ObjectStore), With<SelfPlayer>>,
}

impl RelationWorld<'_, '_> {
    /// The relation functions' inputs, the units through [`Self::unit`].
    pub(crate) fn get(&self) -> Relations<'_> {
        Relations {
            factions: self.factions.as_deref(),
            reputations: &self.reputations,
            me: self.me_guid(),
            roster: self.roster.as_deref(),
            units: self,
        }
    }

    /// The active player's guid (`0x468550`), else its object's.
    pub(crate) fn me_guid(&self) -> Option<u64> {
        self.self_guid
            .as_ref()
            .and_then(|g| g.0)
            .or_else(|| self.self_q.single().ok()?.0.map(|g| g.0))
    }

    /// The unit `guid` names, as the object manager finds it with the unit typemask
    /// (`0x468460`): `None` for a guid it does not hold, or holds as anything but a unit.
    pub(crate) fn unit(&self, guid: u64) -> Option<RelUnit> {
        if self.me_guid() == Some(guid) {
            return self.me();
        }
        let entity = *self.index.as_ref()?.0.get(&guid)?;
        let store = self.stores.get(entity).ok().filter(|s| s.is_unit())?;
        Some(self.unit_of(guid, store))
    }

    /// The active player, once its object has streamed and its guid is known.
    pub(crate) fn me(&self) -> Option<RelUnit> {
        let guid = self.me_guid()?;
        let (_, store) = self.self_q.single().ok()?;
        Some(self.unit_of(guid, store))
    }

    /// The unit `guid` names, whose descriptor the caller already holds.
    pub(crate) fn unit_of(&self, guid: u64, store: &ObjectStore) -> RelUnit {
        let unit = RelUnit::of(guid, store);
        match self.names.as_deref() {
            Some(names) => unit.with_record(store, names),
            None => unit,
        }
    }

    /// Whether an input besides the descriptors moved since the system last ran: the faction
    /// tables, the reputation list, the active player's guid or the party slots.
    pub(crate) fn inputs_changed(&self) -> bool {
        self.reputations.is_changed()
            || self.factions.as_ref().is_some_and(|r| r.is_changed())
            || self.self_guid.as_ref().is_some_and(|r| r.is_changed())
            || self.roster.as_ref().is_some_and(|r| r.is_changed())
    }

    /// Whether a creature record arrived since the system last ran: a record carries the
    /// VISIBLE_TO_GHOSTS bit [`RelUnit::with_record`] reads.
    pub(crate) fn records_changed(&self) -> bool {
        self.names.as_ref().is_some_and(|n| n.is_changed())
    }

    /// The party slots and raid roster as the frame began.
    pub(crate) fn roster(&self) -> Option<&GroupRoster> {
        self.roster.as_deref()
    }

    /// The faction tables.
    pub(crate) fn factions(&self) -> Option<&Factions> {
        self.factions.as_deref()
    }

    /// The reputation list and forced reactions.
    pub(crate) fn reputations(&self) -> &Reputations {
        &self.reputations
    }
}

impl UnitLookup for RelationWorld<'_, '_> {
    fn unit(&self, guid: u64) -> Option<RelUnit> {
        RelationWorld::unit(self, guid)
    }
}

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
    let reaction = (|| {
        let catalog = factions?.catalog();
        let own = catalog.template(self_store?.0.unit_faction_template()?)?;
        Some(own.reaction_toward(catalog.race_template(corpse.0.corpse_race())?))
    })();
    reaction.unwrap_or(Reaction::Neutral) >= Reaction::Friendly
}

mod snapshot;
pub(crate) use snapshot::RelationSnapshot;

#[cfg(test)]
pub(crate) mod fixture;
#[cfg(test)]
mod tests;
