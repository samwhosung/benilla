//! The group relations the party and raid words ask: `BindTarget 0x6e5b40`'s party arm
//! (`6e5cad`-`6e5cec`) needs `0x606c20(caster, unit)` and `CanAssist`, its raid arm
//! (`6e5cf1`-`6e5d2f`) `0x606d20(caster, unit)` and `CanAssist`, and the world pick's party leg
//! (`480686`, `4806b8`) asks `0x606c20`.
//!
//! - [`GroupRoster`]: the guids the reference's two tables hold, and [`publish_group_roster`],
//!   which copies them out of `GroupState` before the input pass, so no cast or hover system
//!   reads that resource and orders against the drains that write it.
//! - [`GroupInputs`]: what a [`TargetRelations`] carries beside its stores for the two
//!   predicates, and [`TargetRelations::in_party`] and [`TargetRelations::in_group`], which ask
//!   them.

use bevy::prelude::*;

use benilla_protocol::messages::{ObjectType, OwnerFallback};

use super::cast_target::TargetRelations;
use crate::net::{ObjectStore, SelfGuid};
use crate::ui_party::{raid_row_guids, GroupState};

/// `UNIT_FIELD_FLAGS & 0x8`, `UNIT_FLAG_PLAYER_CONTROLLED` (vmangos `UnitDefines.h:548`), which
/// both predicates test on both units (`606c5e`, `606c76`, `606d5e`, `606d76`).
const UNIT_FLAG_PLAYER_CONTROLLED: u32 = 0x8;

/// The two tables `0x4e7f70` and `0x4baee0` read: the guids in the party slots and in the raid
/// roster, as the frame began.
#[derive(Resource, Default, Debug, PartialEq, Eq)]
pub(crate) struct GroupRoster {
    /// The party slots `[0xbc6f48 + 8i]`, `i < 4` (`4e7f96`-`4e7fb0`); none once an opcode emptied
    /// them.
    pub(crate) party: Vec<u64>,
    /// The raid roster `[[0xb712a8 + 4i]]`, `i < [0xb713e0]` (`4baee0`), us included; none outside
    /// a raid.
    pub(crate) raid: Vec<u64>,
}

impl GroupRoster {
    /// `0x4e7f70`: a nonzero guid that is the active player's (`4e7f84`) or in a party slot.
    fn in_party(&self, me: Option<u64>, guid: u64) -> bool {
        guid != 0 && (me == Some(guid) || self.party.contains(&guid))
    }

    /// `0x4918e0`: [`Self::in_party`], or a nonzero guid in the raid roster (`4918ee`).
    fn in_group(&self, me: Option<u64>, guid: u64) -> bool {
        self.in_party(me, guid) || guid != 0 && self.raid.contains(&guid)
    }
}

/// Publish [`GroupRoster`] before the input pass, beside the pick's other published inputs. The
/// party slots are the ones `partyN` reads (`GroupState::party_slots`), none after a leave or a
/// kick (`slots_emptied`, `0x4e84a0`); the raid roster is the rows `raidN` reads
/// ([`raid_row_guids`]).
pub(crate) fn publish_group_roster(
    group: Option<Res<GroupState>>,
    self_guid: Res<SelfGuid>,
    mut roster: ResMut<GroupRoster>,
) {
    let Some(group) = group else { return };
    if !group.is_changed() && !self_guid.is_changed() {
        return;
    }
    let party = if group.slots_emptied {
        Vec::new()
    } else {
        group.party_slots().map(|m| m.guid).collect()
    };
    let now = GroupRoster {
        party,
        raid: raid_row_guids(&group, self_guid.0),
    };
    if *roster != now {
        *roster = now;
    }
}

/// What the two predicates read beside a [`TargetRelations`]' stores.
#[derive(Clone, Copy, Default)]
pub(crate) struct GroupInputs<'a> {
    /// The active player's guid (`0x468550`), whom both predicates compare controllers to.
    pub(crate) self_guid: Option<u64>,
    /// The candidate's guid.
    pub(crate) target_guid: Option<u64>,
    /// The caster's charmer or creator store, as [`TargetRelations::target_owner_store`] is the
    /// candidate's.
    pub(crate) self_owner_store: Option<&'a ObjectStore>,
    /// `None` in a harness that publishes none: nobody is anybody's group.
    pub(crate) roster: Option<&'a GroupRoster>,
}

impl TargetRelations<'_> {
    /// `0x606c20(caster, candidate)`: the candidate is the caster (`606c3d`), or a member of his
    /// party, or a unit that answers to one ([`Self::related`]).
    pub(super) fn in_party(&self, is_self: bool) -> bool {
        self.related(is_self, GroupRoster::in_party)
    }

    /// `0x606d20(caster, candidate)`: `0x606c20` with `0x4918e0` in place of `0x4e7f70`, so a
    /// member of the raid roster counts as well as one of the party slots.
    pub(super) fn in_group(&self, is_self: bool) -> bool {
        self.related(is_self, GroupRoster::in_group)
    }

    /// The shape both predicates share: the caster is the candidate, or both are player-controlled
    /// (`606c4f`-`606c79`), each answers to a player ([`controller`]), and one of the two
    /// controllers is the active player and the other a member (`606ca3`-`606d02`), tried both
    /// ways round. A member's pet is such a unit. Whoever has not streamed answers no controller
    /// and no relation.
    fn related(&self, is_self: bool, member: fn(&GroupRoster, Option<u64>, u64) -> bool) -> bool {
        if is_self {
            return true;
        }
        let group = &self.group;
        let (Some(me), Some(target), Some(me_store), Some(store)) = (
            group.self_guid,
            group.target_guid,
            self.self_store,
            self.target_store,
        ) else {
            return false;
        };
        if me_store.0.unit_flags() & store.0.unit_flags() & UNIT_FLAG_PLAYER_CONTROLLED == 0 {
            return false;
        }
        let (Some(mine), Some(theirs)) = (
            controller(me_store, me, group.self_owner_store),
            controller(store, target, self.target_owner_store),
        ) else {
            return false;
        };
        let member = |guid| {
            group
                .roster
                .is_some_and(|roster| member(roster, Some(me), guid))
        };
        mine == me && member(theirs) || theirs == me && member(mine)
    }
}

/// `0x606170`: the player a unit answers to, its charmer else its creator (`606170`-`60619f`)
/// or the unit itself when neither is set, and only when that object is a player (`6061c6`-
/// `6061cf`, `TYPEMASK_PLAYER`). `owner_store` is the charmer's or creator's descriptor, `None`
/// while it has not streamed.
fn controller(store: &ObjectStore, guid: u64, owner_store: Option<&ObjectStore>) -> Option<u64> {
    let (owner, owner_store) = match store.0.unit_owner(OwnerFallback::CreatedBy) {
        Some(owner) => (owner, owner_store),
        None => (guid, Some(store)),
    };
    owner_store
        .is_some_and(|s| s.0.object_type() == Some(ObjectType::Player))
        .then_some(owner)
}

/// The stores the group tests stand up, over the faction rows of [`super::targeting::corpse_fixture`]
/// (template 1 Human, the caster's, and 2 Orc).
#[cfg(test)]
pub(crate) mod fixture {
    use super::*;
    use benilla_protocol::ObjectFields;

    /// Absolute descriptor indices: `OBJECT_FIELD_TYPE`, `UNIT_FIELD_CHARMEDBY`,
    /// `UNIT_FIELD_CREATEDBY`, `UNIT_FIELD_HEALTH`, `UNIT_FIELD_FACTIONTEMPLATE` and
    /// `UNIT_FIELD_FLAGS`.
    pub(crate) const TYPE: u16 = 2;
    pub(crate) const CHARMED_BY: u16 = 10;
    pub(crate) const CREATED_BY: u16 = 14;
    pub(crate) const HEALTH: u16 = 22;
    pub(crate) const TEMPLATE: u16 = 35;
    pub(crate) const FLAGS: u16 = 46;
    /// `TYPEMASK_OBJECT | UNIT | PLAYER`, and `TYPEMASK_OBJECT | UNIT`.
    pub(crate) const PLAYER_TYPE: u32 = 0x19;
    pub(crate) const CREATURE_TYPE: u32 = 0x09;
    /// `UNIT_FLAG_PLAYER_CONTROLLED`, and `UNIT_FLAG_PVP` (`0x1000`), which makes a friendly
    /// creature assistable.
    pub(crate) const CONTROLLED: u32 = 0x8;
    pub(crate) const PVP: u32 = 0x1000;

    pub(crate) fn store(pairs: &[(u16, u32)]) -> ObjectStore {
        ObjectStore(ObjectFields::from_pairs(pairs))
    }

    /// A living, player-controlled player of this faction template.
    pub(crate) fn player(template: u32) -> ObjectStore {
        store(&[
            (TYPE, PLAYER_TYPE),
            (TEMPLATE, template),
            (FLAGS, CONTROLLED),
            (HEALTH, 100),
        ])
    }

    /// A living, player-controlled creature of this template that answers to `owner`, as a pet
    /// does: its creator, or its charmer when `charmed`.
    pub(crate) fn pet_of(owner: u64, template: u32, charmed: bool) -> ObjectStore {
        store(&[
            (TYPE, CREATURE_TYPE),
            (TEMPLATE, template),
            (FLAGS, CONTROLLED),
            (HEALTH, 100),
            (if charmed { CHARMED_BY } else { CREATED_BY }, owner as u32),
        ])
    }

    /// A living creature of this template that nobody controls, with these unit flags.
    pub(crate) fn npc(template: u32, flags: u32) -> ObjectStore {
        store(&[
            (TYPE, CREATURE_TYPE),
            (TEMPLATE, template),
            (FLAGS, flags),
            (HEALTH, 100),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::*;
    use super::*;
    use crate::net::Reputations;
    use crate::spell::cast_target::{
        cast_target_mask, resolve_cast_target, unit_binds, CastCandidates, CastWireTarget,
    };
    use crate::spell::targeting::corpse_fixture as fx;
    use benilla_formats::SpellDisplay;
    use benilla_protocol::messages::GroupMemberEntry;
    use bevy::ecs::system::RunSystemOnce;

    const ME: u64 = 1;
    const MEMBER: u64 = 2;
    const RAIDER: u64 = 3;
    const STRANGER: u64 = 4;
    const MEMBER_PET: u64 = 5;
    const NPC: u64 = 6;
    const ORC_MEMBER: u64 = 7;

    /// `TARGET_FLAG_UNIT_RAID` and `TARGET_FLAG_UNIT_PARTY`, the words implicit targets 57 and 35
    /// set.
    const RAID_WORD: u16 = 0x4;
    const PARTY_WORD: u16 = 0x8;

    /// Us and our party of one, a raider in our raid and a stranger in neither.
    fn roster() -> GroupRoster {
        GroupRoster {
            party: vec![MEMBER],
            raid: vec![ME, MEMBER, RAIDER],
        }
    }

    /// `(0x606c20, 0x606d20)` toward a candidate, with the stores that stand behind it.
    fn asked(
        roster: &GroupRoster,
        me: &ObjectStore,
        me_owner: Option<&ObjectStore>,
        target: (u64, &ObjectStore, Option<&ObjectStore>),
    ) -> (bool, bool) {
        let (guid, store, owner) = target;
        let rel = TargetRelations {
            target_store: Some(store),
            target_owner_store: owner,
            self_store: Some(me),
            factions: None,
            reputations: &Reputations(Vec::new()),
            types: Default::default(),
            group: GroupInputs {
                self_guid: Some(ME),
                target_guid: Some(guid),
                self_owner_store: me_owner,
                roster: Some(roster),
            },
        };
        (rel.in_party(false), rel.in_group(false))
    }

    /// `0x606c20` and `0x606d20` on players: the party slots satisfy both, the raid roster only the
    /// second, and a player in neither answers neither.
    #[test]
    fn the_raid_roster_adds_to_the_party_slots_for_the_second_predicate_alone() {
        let (roster, me, other) = (roster(), player(1), player(1));
        let ask = |guid| asked(&roster, &me, None, (guid, &other, None));
        assert_eq!(ask(MEMBER), (true, true), "a party member");
        assert_eq!(ask(RAIDER), (false, true), "a raid member of another group");
        assert_eq!(ask(STRANGER), (false, false), "a player in neither");
        assert_eq!(
            ask(0),
            (false, false),
            "no guid is nobody's (`4e7f75`, `4baee9`)"
        );
        assert_eq!(
            asked(&GroupRoster::default(), &me, None, (MEMBER, &other, None)),
            (false, false),
            "an empty roster"
        );
    }

    /// The caster is the candidate (`606c3d`, `606d3d`) whatever the roster holds, and a candidate
    /// with no guid or no descriptor streamed answers no relation.
    #[test]
    fn the_caster_is_his_own_group_and_a_missing_input_is_none() {
        let (me, other) = (player(1), player(1));
        let roster = roster();
        let rel = TargetRelations {
            target_store: Some(&other),
            target_owner_store: None,
            self_store: Some(&me),
            factions: None,
            reputations: &Reputations(Vec::new()),
            types: Default::default(),
            group: GroupInputs {
                self_guid: Some(ME),
                target_guid: Some(MEMBER),
                self_owner_store: None,
                roster: Some(&roster),
            },
        };
        assert!(rel.in_party(true) && rel.in_group(true), "the caster");
        assert!(rel.in_party(false) && rel.in_group(false), "the control");
        let no_guid = TargetRelations {
            group: GroupInputs {
                target_guid: None,
                ..rel.group
            },
            ..rel
        };
        assert!(!no_guid.in_party(false) && !no_guid.in_group(false));
        let no_store = TargetRelations {
            target_store: None,
            ..rel
        };
        assert!(!no_store.in_party(false) && !no_store.in_group(false));
        let no_roster = TargetRelations {
            group: GroupInputs {
                roster: None,
                ..rel.group
            },
            ..rel
        };
        assert!(!no_roster.in_party(false) && !no_roster.in_group(false));
    }

    /// `606c5e`-`606c76`: both units must be player-controlled, so a member who is not, or a caster
    /// who is not, has no party.
    #[test]
    fn both_sides_must_be_player_controlled() {
        let roster = roster();
        let (me, member) = (player(1), player(1));
        let unflagged = store(&[(TYPE, PLAYER_TYPE)]);
        assert_eq!(
            asked(&roster, &me, None, (MEMBER, &unflagged, None)),
            (false, false)
        );
        assert_eq!(
            asked(&roster, &unflagged, None, (MEMBER, &member, None)),
            (false, false)
        );
    }

    /// `0x606170`: a pet is the group member it answers to, so a party member's pet is in the party
    /// and a raider's in the raid; a pet of a stranger, a creature that answers to nobody and a pet
    /// whose owner has not streamed are in neither.
    #[test]
    fn a_pet_is_the_group_member_it_answers_to() {
        let roster = roster();
        let (me, member, raider, stranger) = (player(1), player(1), player(1), player(1));
        let ask = |guid: u64, pet: &ObjectStore, owner: Option<&ObjectStore>| {
            asked(&roster, &me, None, (guid, pet, owner))
        };
        assert_eq!(
            ask(10, &pet_of(MEMBER, 1, false), Some(&member)),
            (true, true),
            "a member's pet"
        );
        assert_eq!(
            ask(11, &pet_of(ME, 1, false), Some(&me)),
            (true, true),
            "our own pet"
        );
        assert_eq!(
            ask(12, &pet_of(RAIDER, 1, false), Some(&raider)),
            (false, true),
            "a raider's pet"
        );
        assert_eq!(
            ask(13, &pet_of(STRANGER, 1, false), Some(&stranger)),
            (false, false),
            "a stranger's pet"
        );
        assert_eq!(
            ask(14, &pet_of(MEMBER, 1, false), None),
            (false, false),
            "an owner that has not streamed answers no controller (`606c92`)"
        );
        // A creature nobody controls: the flag says player-controlled, but it is no player.
        let orphan = store(&[(TYPE, CREATURE_TYPE), (FLAGS, CONTROLLED)]);
        assert_eq!(
            ask(15, &orphan, None),
            (false, false),
            "no owner, no player"
        );
        assert_eq!(
            ask(16, &npc(1, PVP), None),
            (false, false),
            "an NPC is not player-controlled"
        );
        // Only a player counts as a controller (`6061c6`-`6061cf`): a creature standing in the
        // roster, and a pet that answers to one, have none.
        let controlled = store(&[(TYPE, CREATURE_TYPE), (FLAGS, CONTROLLED)]);
        assert_eq!(
            ask(MEMBER, &controlled, None),
            (false, false),
            "a controlled creature whose own guid is in the party"
        );
        assert_eq!(
            ask(18, &pet_of(MEMBER, 1, false), Some(&controlled)),
            (false, false),
            "a pet whose owner in the party is no player"
        );
        // A pet answers to its charmer before its creator: `CHARMEDBY` decides.
        let charmed = store(&[
            (TYPE, CREATURE_TYPE),
            (FLAGS, CONTROLLED),
            (CHARMED_BY, STRANGER as u32),
            (CREATED_BY, MEMBER as u32),
        ]);
        assert_eq!(
            ask(17, &charmed, Some(&stranger)),
            (false, false),
            "charmed by a stranger"
        );
    }

    /// `606dce`-`606e02`: the controllers are tried both ways round, so a caster who answers to a
    /// member (a charmed player) is in the group of a unit that answers to us.
    #[test]
    fn the_controllers_are_tried_both_ways_round() {
        let roster = roster();
        let (member, stranger) = (player(1), player(1));
        let charmed_by = |guid: u64| {
            store(&[
                (TYPE, PLAYER_TYPE),
                (FLAGS, CONTROLLED),
                (CHARMED_BY, guid as u32),
            ])
        };
        // Our pet answers to us (its creator is the active player) and we answer to a member: the
        // caster's controller is not the active player, the candidate's is.
        let (charmed_me, ours) = (charmed_by(MEMBER), pet_of(ME, 1, false));
        assert_eq!(
            asked(
                &roster,
                &charmed_me,
                Some(&member),
                (10, &ours, Some(&charmed_me))
            ),
            (true, true)
        );
        // Charmed by a stranger instead: the other side is no member.
        let charmed_by_stranger = charmed_by(STRANGER);
        assert_eq!(
            asked(
                &roster,
                &charmed_by_stranger,
                Some(&stranger),
                (10, &ours, Some(&charmed_by_stranger))
            ),
            (false, false)
        );
        // Neither controller is the active player: no relation, whatever they are.
        let theirs = pet_of(RAIDER, 1, false);
        assert_eq!(
            asked(
                &roster,
                &charmed_me,
                Some(&member),
                (11, &theirs, Some(&member))
            ),
            (false, false)
        );
    }

    /// The party bit `0x8` and the raid bit `0x4` clear for the group the predicates name and
    /// `CanAssist` passes (`6e5cad`-`6e5d2f`), and only the raid bit for a raid member outside
    /// the party.
    #[test]
    fn the_party_and_raid_bits_clear_for_the_group_and_only_when_assistable() {
        let factions = fx::factions();
        let reputations = Reputations(Vec::new());
        let roster = GroupRoster {
            party: vec![MEMBER, ORC_MEMBER],
            raid: vec![ME, MEMBER, RAIDER, ORC_MEMBER],
        };
        let me = player(1);
        let (member, raider, stranger) = (player(1), player(1), player(1));
        let orc_member = player(2);
        let member_pet = pet_of(MEMBER, 1, false);
        // A friendly creature that can be assisted but answers to nobody.
        let npc = npc(1, PVP);
        let binds = |word: u16, is_self: bool, guid, store: &ObjectStore, owner| {
            let rel = TargetRelations {
                target_store: Some(store),
                target_owner_store: owner,
                self_store: Some(&me),
                factions: Some(&factions),
                reputations: &reputations,
                types: Default::default(),
                group: GroupInputs {
                    self_guid: Some(ME),
                    target_guid: Some(guid),
                    self_owner_store: None,
                    roster: Some(&roster),
                },
            };
            unit_binds(None, word, is_self, &rel)
        };
        // (label, guid, store, owner, binds under the party word, under the raid word)
        type Case<'a> = (
            &'a str,
            u64,
            &'a ObjectStore,
            Option<&'a ObjectStore>,
            bool,
            bool,
        );
        let cases: [Case; 6] = [
            ("a party member", MEMBER, &member, None, true, true),
            (
                "a raid member of another group",
                RAIDER,
                &raider,
                None,
                false,
                true,
            ),
            (
                "a player in neither",
                STRANGER,
                &stranger,
                None,
                false,
                false,
            ),
            (
                "a party member's pet",
                MEMBER_PET,
                &member_pet,
                Some(&member),
                true,
                true,
            ),
            ("a friendly creature", NPC, &npc, None, false, false),
            // In the party, and no friend: `CanAssist` refuses after the predicate passes.
            (
                "a member of the other faction",
                ORC_MEMBER,
                &orc_member,
                None,
                false,
                false,
            ),
        ];
        for (label, guid, store, owner, party, raid) in cases {
            assert_eq!(
                binds(PARTY_WORD, false, guid, store, owner),
                party,
                "{label}: the party word"
            );
            assert_eq!(
                binds(RAID_WORD, false, guid, store, owner),
                raid,
                "{label}: the raid word"
            );
        }
        // The caster binds both, in no group at all.
        for word in [PARTY_WORD, RAID_WORD] {
            assert!(binds(word, true, ME, &me, None), "the caster, {word:#x}");
        }
    }

    /// Power Word: Shield (implicit target 57, the raid word) on the press: a party member binds
    /// and goes on the wire, a friendly player in no group does not, and the fallback to ourselves
    /// is `autoSelfCast`'s.
    #[test]
    fn a_raid_word_spell_pressed_on_a_party_member_binds_him() {
        let factions = fx::factions();
        let reputations = Reputations(Vec::new());
        let roster = GroupRoster {
            party: vec![MEMBER],
            raid: Vec::new(),
        };
        let (me, member, stranger) = (player(1), player(1), player(1));
        let shield = SpellDisplay {
            implicit_target_a1: 57,
            ..Default::default()
        };
        assert_eq!(cast_target_mask(&shield), RAID_WORD);
        let press = |target: (u64, &ObjectStore), auto_self_cast| {
            let rel = TargetRelations {
                target_store: Some(target.1),
                target_owner_store: None,
                self_store: Some(&me),
                factions: Some(&factions),
                reputations: &reputations,
                types: Default::default(),
                group: GroupInputs {
                    self_guid: Some(ME),
                    target_guid: Some(target.0),
                    self_owner_store: None,
                    roster: Some(&roster),
                },
            };
            let candidates = CastCandidates {
                selection: Some(target.0),
                caster: Some(ME),
                main_hand_item: None,
            };
            resolve_cast_target(Some(&shield), &candidates, auto_self_cast, &rel)
        };
        for auto_self_cast in [false, true] {
            assert_eq!(
                press((MEMBER, &member), auto_self_cast),
                CastWireTarget::Unit(MEMBER),
                "a party member, autoSelfCast {auto_self_cast}"
            );
        }
        assert_eq!(
            press((STRANGER, &stranger), false),
            CastWireTarget::Targeting(RAID_WORD),
            "a stranger leaves the word standing for the cursor"
        );
        assert_eq!(
            press((STRANGER, &stranger), true),
            CastWireTarget::Unit(ME),
            "and autoSelfCast falls back to the caster"
        );
    }

    /// The shipped rows the party and raid words serve: each carries its bit, and a party member
    /// selected when the row is pressed is the bound target, a stranger not.
    #[test]
    fn the_shipped_party_and_raid_spells_bind_a_party_member() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let catalog = benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc");
        let factions = fx::factions();
        let reputations = Reputations(Vec::new());
        let roster = GroupRoster {
            party: vec![MEMBER],
            raid: Vec::new(),
        };
        let (me, member, stranger) = (player(1), player(1), player(1));
        // The party word (target 35), then the raid word (targets 57 and 61).
        let rows = [
            (13903, "Seal of Sacrifice", PARTY_WORD),
            (17177, "Seal of Protection", PARTY_WORD),
            (17, "Power Word: Shield", RAID_WORD),
            (1022, "Blessing of Protection", RAID_WORD),
            (1038, "Blessing of Salvation", RAID_WORD),
            (6940, "Blessing of Sacrifice", RAID_WORD),
            (604, "Dampen Magic", RAID_WORD),
            (1008, "Amplify Magic", RAID_WORD),
            (19752, "Divine Intervention", RAID_WORD),
            (25782, "Greater Blessing of Might", RAID_WORD),
            (25898, "Greater Blessing of Kings", RAID_WORD),
        ];
        for (id, name, bit) in rows {
            let def = catalog.get(id).unwrap_or_else(|| panic!("{name} ({id})"));
            assert_eq!(def.name, name, "spell {id}");
            assert_eq!(cast_target_mask(def), bit, "{name}: the word");
            let press = |target: (u64, &ObjectStore)| {
                let rel = TargetRelations {
                    target_store: Some(target.1),
                    target_owner_store: None,
                    self_store: Some(&me),
                    factions: Some(&factions),
                    reputations: &reputations,
                    types: Default::default(),
                    group: GroupInputs {
                        self_guid: Some(ME),
                        target_guid: Some(target.0),
                        self_owner_store: None,
                        roster: Some(&roster),
                    },
                };
                let candidates = CastCandidates {
                    selection: Some(target.0),
                    caster: Some(ME),
                    main_hand_item: None,
                };
                resolve_cast_target(Some(def), &candidates, false, &rel)
            };
            assert_eq!(
                press((MEMBER, &member)),
                CastWireTarget::Unit(MEMBER),
                "{name}: a party member"
            );
            assert_eq!(
                press((STRANGER, &stranger)),
                CastWireTarget::Targeting(bit),
                "{name}: a stranger"
            );
        }
    }

    fn entry(guid: u64, flags: u8) -> GroupMemberEntry {
        GroupMemberEntry {
            name: String::new(),
            guid,
            status: 0,
            flags,
        }
    }

    /// The published tables: the party slots are our own subgroup, at most four, and the raid
    /// roster every row, us first, none outside a raid. A leave or a kick empties the slots and
    /// keeps the roster.
    #[test]
    fn the_roster_publishes_the_slots_and_rows_the_reference_holds() {
        let publish = |world: &mut World| {
            world
                .run_system_once(publish_group_roster)
                .expect("the roster publishes");
            std::mem::take(&mut *world.resource_mut::<GroupRoster>())
        };
        let mut world = World::new();
        world.init_resource::<GroupRoster>();
        world.init_resource::<GroupState>();
        world.insert_resource(SelfGuid(Some(ME)));
        assert_eq!(publish(&mut world), GroupRoster::default(), "no group");

        // A party: everyone in our subgroup, and no raid roster.
        world.resource_mut::<GroupState>().members =
            vec![entry(MEMBER, 0), entry(RAIDER, 0), entry(STRANGER, 0)];
        assert_eq!(
            publish(&mut world),
            GroupRoster {
                party: vec![MEMBER, RAIDER, STRANGER],
                raid: Vec::new(),
            }
        );

        // A raid: the party slots are our subgroup's, the roster all rows with us first.
        {
            let mut group = world.resource_mut::<GroupState>();
            group.group_type = crate::ui_party::GROUPTYPE_RAID;
            group.members = vec![entry(MEMBER, 0), entry(RAIDER, 1), entry(STRANGER, 0)];
        }
        assert_eq!(
            publish(&mut world),
            GroupRoster {
                party: vec![MEMBER, STRANGER],
                raid: vec![ME, MEMBER, RAIDER, STRANGER],
            }
        );

        // A kick empties the slots and keeps the roster (`0x4e84a0`).
        world.resource_mut::<GroupState>().slots_emptied = true;
        assert_eq!(
            publish(&mut world),
            GroupRoster {
                party: Vec::new(),
                raid: vec![ME, MEMBER, RAIDER, STRANGER],
            }
        );
    }
}
