//! The world pick's unit and player filter while a word stands. The pick flags come from the
//! word alone (`0x481050`'s targeting arm, `4810f1`-`481187`), and each unit and player the trace
//! finds meets them in `0x480610`, where `0x480780`'s unit and player arms send it. A unit the
//! spell cannot take is no candidate, so the cursor and the click reach the one behind it.
//!
//! - [`PickFlags`]: the unit-shaped bits of the flags, [`PickFlags::of`], and the legs of
//!   `0x480610` they switch on, [`PickFlags::admits`].
//! - [`UnitPick`]: the flags as the frame began, [`publish_unit_pick`], beside
//!   [`super::PicksSelf`] (flag `0x20`) and [`super::CorpsePick`] (flag `0x40`), which own the
//!   other two bits the flags carry for these arms.
//! - [`PickChecks`]: what the hover reads per candidate, the relation checks the bind runs
//!   ([`crate::spell::cast_target::TargetRelations`]), the party leg `0x606c20` among them.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::SpellTargeting;
use crate::net::{Guid, GuidIndex, ObjectStore, Reputations, SelfGuid, SelfPlayer};
use crate::spell::bind_gates::unit_alive;
use crate::spell::cast_target::{owner_store, TargetRelations};
use crate::spell::group_relation::{GroupInputs, GroupRoster};
use crate::target::Factions;

/// Flags `0x8` and `0x10`, which `0x480780`'s unit and player arms test (`4807e1`, `4807cb`) and
/// `0x481050` sets together (`481116 or esi,0x18`) for a word with unit bits.
const UNITS: u32 = 0x18;
/// The relation flags: `0x606c20` (`0x6e61e0`), `CanAssist 0x6066f0` (`0x6e6200`) and `CanAttack
/// 0x606980` (`0x6e6210`), of which `0x480610` needs one (`480686`).
const PARTY: u32 = 0x1_0000;
const ASSIST: u32 = 0x2_0000;
const ATTACK: u32 = 0x4_0000;
const RELATIONS: u32 = PARTY | ASSIST | ATTACK;
/// The liveness flags: the word takes a living candidate, or a dead one (`4806fd`).
const ALIVE: u32 = 0x10_0000;
const DEAD: u32 = 0x20_0000;
const LIVENESS: u32 = ALIVE | DEAD;

/// `UNIT_FLAG_IMMUNE_TO_PLAYER` (vmangos `UnitDefines.h:553`): the `[desc+0xa0]` bit 8 that
/// `0x480737` refuses while targeting.
const UNIT_FLAG_IMMUNE_TO_PLAYER: u32 = 0x100;

/// The unit-shaped bits of the pick flags `0x481050` builds from the standing word. The terrain
/// (`0x3`), GameObject (`0x4`), self (`0x20`) and corpse (`0x40`) bits belong to their own arms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PickFlags(u32);

impl PickFlags {
    /// `0x481050`'s targeting arm over the word `0xcecac0`:
    ///
    /// - units and players (`0x18`) for a word in `0x878e` (`0x6e6180`), and inside that arm the
    ///   relation flags for a word in `0x40c` (`0x6e61e0`, party), `0x500` (`0x6e6200`, assist)
    ///   and `0x480` (`0x6e6210`, enemy), then the liveness flag: dead for a word in `0x8600`
    ///   (`0x6e6230`, with the corpse flag, `0x200040`), else alive (`0x100000`);
    /// - after it, the ally corpse bit `0x8000` (`0x6e6240`) adds the assist flag and the enemy
    ///   corpse bit `0x200` (`0x6e6250`) the attack flag (`0x20040`, `0x40040`).
    pub(crate) fn of(word: u16) -> Self {
        let word = u32::from(word);
        let mut flags = 0;
        if word & 0x878e != 0 {
            flags |= UNITS;
            if word & 0x040c != 0 {
                flags |= PARTY;
            }
            if word & 0x0500 != 0 {
                flags |= ASSIST;
            }
            if word & 0x0480 != 0 {
                flags |= ATTACK;
            }
            flags |= if word & 0x8600 != 0 { DEAD } else { ALIVE };
        }
        if word & 0x8000 != 0 {
            flags |= ASSIST;
        }
        if word & 0x0200 != 0 {
            flags |= ATTACK;
        }
        Self(flags)
    }

    /// Whether a unit or player is a candidate of the pick: `0x480780`'s arms need the unit
    /// flags, then `0x480610`'s three legs:
    ///
    /// - `480686`: with a relation flag set, one of the set relations must hold. The party leg
    ///   `in_party` is `0x606c20`, run only when asked.
    /// - `4806fd`: with a liveness flag set, a living candidate needs `0x100000` and a dead one
    ///   `0x200000`. Alive is the signed `UNIT_FIELD_HEALTH` above 0 (`480713 setg`), the bind's
    ///   own test ([`unit_alive`]), and a candidate with no descriptor reads dead.
    /// - `480737`: while targeting, `UNIT_FIELD_FLAGS & 0x100` refuses.
    ///
    /// The self exclusions that come first (`48062c`-`480683`) are the caller's.
    pub(crate) fn admits(
        self,
        is_self: bool,
        rel: &TargetRelations,
        in_party: impl FnOnce() -> bool,
    ) -> bool {
        let flags = self.0;
        if flags & UNITS == 0 {
            return false;
        }
        let related = flags & RELATIONS == 0
            || flags & PARTY != 0 && in_party()
            || flags & ASSIST != 0 && rel.assistable(is_self)
            || flags & ATTACK != 0 && rel.attackable(is_self);
        if !related {
            return false;
        }
        if flags & LIVENESS != 0 {
            let alive = rel.target_store.is_some_and(|s| unit_alive(&s.0));
            let needed = if alive { ALIVE } else { DEAD };
            if flags & needed == 0 {
                return false;
            }
        }
        rel.target_store
            .is_none_or(|s| s.0.unit_flags() & UNIT_FLAG_IMMUNE_TO_PLAYER == 0)
    }
}

/// The unit pick's flags as the frame began, which [`crate::target`]'s hover reads: `None`
/// outside targeting, where they carry `0x5c` and the filter's three legs never run. The press
/// latches the hover's pick, so both read this.
#[derive(Resource, Default)]
pub(crate) struct UnitPick {
    pub(crate) flags: Option<PickFlags>,
}

/// Publish [`UnitPick`] before the input pass, beside [`super::PicksSelf`].
pub(crate) fn publish_unit_pick(targeting: Res<SpellTargeting>, mut pick: ResMut<UnitPick>) {
    let flags = targeting
        .0
        .as_ref()
        .map(|targeting| PickFlags::of(targeting.word));
    if pick.flags != flags {
        pick.flags = flags;
    }
}

/// What the hover asks of a unit or player candidate while a word stands: the bind's relation
/// checks over the same inputs, and the party leg.
#[derive(SystemParam)]
pub(crate) struct PickChecks<'w, 's> {
    pick: Res<'w, UnitPick>,
    stores: Query<'w, 's, &'static ObjectStore>,
    index: Option<Res<'w, GuidIndex>>,
    self_q: Query<'w, 's, (Entity, Option<&'static ObjectStore>), With<SelfPlayer>>,
    factions: Option<Res<'w, Factions>>,
    reputations: Res<'w, Reputations>,
    self_guid: Res<'w, SelfGuid>,
    guids: Query<'w, 's, &'static Guid>,
    /// The party slots and raid roster as the frame began; a harness without them has no group.
    roster: Option<Res<'w, GroupRoster>>,
}

impl PickChecks<'_, '_> {
    /// Whether this unit or player is a candidate of the pick: always outside targeting,
    /// otherwise [`PickFlags::admits`]. `store` is its descriptor, if streamed.
    pub(crate) fn admits_unit(&self, entity: Entity, store: Option<&ObjectStore>) -> bool {
        let Some(flags) = self.pick.flags else {
            return true;
        };
        let is_self = self
            .self_q
            .iter()
            .next()
            .is_some_and(|(me, _)| me == entity);
        let self_store = self.self_q.iter().next().and_then(|(_, store)| store);
        let rel = TargetRelations {
            target_store: store,
            // `CanAssist`'s `IsPvP` owner chase: charmer, else creator.
            target_owner_store: owner_store(store, self.index.as_deref(), &self.stores),
            self_store,
            factions: self.factions.as_deref(),
            reputations: &self.reputations,
            // The pick reads no creature type.
            types: Default::default(),
            group: GroupInputs {
                self_guid: self.self_guid.0,
                target_guid: self.guids.get(entity).ok().map(|guid| guid.0),
                self_owner_store: owner_store(self_store, self.index.as_deref(), &self.stores),
                roster: self.roster.as_deref(),
            },
        };
        flags.admits(is_self, &rel, || rel.in_party(is_self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::{GuidIndex, Reputations};
    use crate::spell::group_relation::publish_group_roster;
    use crate::spell::targeting::corpse_fixture as fx;
    use benilla_protocol::messages::GroupMemberEntry;
    use benilla_protocol::ObjectFields;
    use bevy::ecs::system::RunSystemOnce;

    /// Absolute descriptor indices: `OBJECT_FIELD_TYPE`, `UNIT_FIELD_HEALTH`,
    /// `UNIT_FIELD_FLAGS`, `UNIT_FIELD_CHARMEDBY` and `UNIT_FIELD_CREATEDBY`.
    const TYPE: u16 = 2;
    const HEALTH: u16 = 22;
    const FLAGS: u16 = 46;
    const CHARMED_BY: u16 = 10;
    const CREATED_BY: u16 = 14;
    /// `UNIT_FIELD_FACTIONTEMPLATE`, on [`fx::factions`]' rows 1 (Human) and 2 (Orc).
    const TEMPLATE: u16 = 35;
    const PLAYER_TYPE: u32 = 0x19;

    // The words: `TARGET_FLAG_*`.
    const UNIT: u16 = 0x0002;
    const RAID: u16 = 0x0004;
    const PARTY_WORD: u16 = 0x0008;
    const ENEMY: u16 = 0x0080;
    const ASSIST_WORD: u16 = 0x0100;
    const CORPSE_ENEMY: u16 = 0x0200;
    const DEAD_ONLY: u16 = 0x0400;
    const CORPSE_ALLY: u16 = 0x8000;

    fn unit(pairs: &[(u16, u32)]) -> ObjectStore {
        let mut pairs = pairs.to_vec();
        if !pairs.iter().any(|&(index, _)| index == HEALTH) {
            pairs.push((HEALTH, 100));
        }
        ObjectStore(ObjectFields::from_pairs(&pairs))
    }

    /// A player of the caster's faction, which `CanAssist` passes on its player-controlled arm.
    fn friend(health: u32) -> ObjectStore {
        friend_with(health, 0)
    }

    fn friend_with(health: u32, flags: u32) -> ObjectStore {
        unit(&[(TEMPLATE, 1), (FLAGS, 0x8 | flags), (HEALTH, health)])
    }

    /// An Orc creature: hostile to the human caster and not player-controlled.
    fn foe(health: u32) -> ObjectStore {
        foe_with(health, 0)
    }

    fn foe_with(health: u32, flags: u32) -> ObjectStore {
        unit(&[(TEMPLATE, 2), (FLAGS, flags), (HEALTH, health)])
    }

    fn admits(word: u16, candidate: &ObjectStore) -> bool {
        admits_as(word, false, candidate, || false)
    }

    fn admits_as(
        word: u16,
        is_self: bool,
        candidate: &ObjectStore,
        in_party: impl FnOnce() -> bool,
    ) -> bool {
        let factions = fx::factions();
        let me = fx::caster(1.5);
        let rel = TargetRelations {
            target_store: Some(candidate),
            target_owner_store: None,
            self_store: Some(&me),
            factions: Some(&factions),
            reputations: &Reputations(Vec::new()),
            types: Default::default(),
            group: Default::default(),
        };
        PickFlags::of(word).admits(is_self, &rel, in_party)
    }

    /// `0x481050`'s targeting arm, bit by bit: which unit-shaped flags each word sets.
    #[test]
    fn the_word_builds_the_flags_as_the_chooser_does() {
        let flags = |word| PickFlags::of(word).0;
        assert_eq!(flags(0), 0, "no word, no unit flag");
        for word in [0x0010, 0x0020, 0x0040, 0x4000, 0x0800, 0x4800] {
            assert_eq!(
                flags(word),
                0,
                "{word:#06x} has no unit bit: no unit is picked"
            );
        }
        assert_eq!(flags(UNIT), UNITS | ALIVE);
        assert_eq!(flags(ASSIST_WORD), UNITS | ASSIST | ALIVE);
        assert_eq!(flags(ENEMY), UNITS | ATTACK | ALIVE);
        assert_eq!(flags(PARTY_WORD), UNITS | PARTY | ALIVE);
        assert_eq!(
            flags(RAID),
            UNITS | PARTY | ALIVE,
            "raid asks `0x606c20` too"
        );
        // Resurrection: the ally corpse bit takes the assist flag through `0x6e6240`, and a word
        // in `0x8600` takes the dead flag instead of the alive one.
        assert_eq!(flags(CORPSE_ALLY), UNITS | ASSIST | DEAD);
        assert_eq!(flags(CORPSE_ENEMY), UNITS | ATTACK | DEAD);
        // Skinning's word: all three relations, and dead.
        assert_eq!(
            flags(UNIT | DEAD_ONLY),
            UNITS | PARTY | ASSIST | ATTACK | DEAD
        );
        assert_eq!(flags(ASSIST_WORD | ENEMY), UNITS | ASSIST | ATTACK | ALIVE);
    }

    /// A heal (the assist word) takes a friend and drops a foe.
    #[test]
    fn an_assist_word_drops_the_hostile_unit_and_keeps_the_friendly_one() {
        assert!(admits(ASSIST_WORD, &friend(100)));
        assert!(!admits(ASSIST_WORD, &foe(100)));
        // A plain unit word has no relation flag: a foe is a candidate.
        assert!(admits(UNIT, &foe(100)));
        assert!(admits(UNIT, &friend(100)));
    }

    /// An attack word (the enemy bit) takes a foe and drops a friend.
    #[test]
    fn an_enemy_word_drops_the_friendly_unit_and_keeps_the_hostile_one() {
        assert!(admits(ENEMY, &foe(100)));
        assert!(!admits(ENEMY, &friend(100)));
        // Neither is the caster's own body: never attackable, so never a candidate.
        assert!(!admits_as(ENEMY, true, &friend(100), || false));
    }

    /// `4806fd`: a living candidate needs the alive flag, a dead one the dead flag.
    #[test]
    fn a_resurrection_word_drops_the_living_and_keeps_the_dead() {
        assert!(admits(CORPSE_ALLY, &friend(0)), "a dead friend");
        assert!(!admits(CORPSE_ALLY, &friend(100)), "a living friend");
        // The assist flag `0x6e6240` adds still applies: a dead foe is no ally's corpse.
        assert!(!admits(CORPSE_ALLY, &foe(0)), "a dead foe");
        // The dead-only word: a dead unit of any relation, never a living one.
        assert!(admits(UNIT | DEAD_ONLY, &foe(0)));
        assert!(admits(UNIT | DEAD_ONLY, &friend(0)));
        assert!(!admits(UNIT | DEAD_ONLY, &foe(100)));
        // Every other unit word takes the living only.
        assert!(
            !admits(ASSIST_WORD, &friend(0)),
            "a heal never takes a corpse"
        );
        assert!(!admits(UNIT, &foe(0)));
        // A candidate with no health streamed is dead, as in the bind.
        let bare = unit(&[(TEMPLATE, 1), (FLAGS, 0x8), (HEALTH, 0)]);
        assert!(admits(CORPSE_ALLY, &bare));
    }

    /// The enemy corpse bit: `0x6e6250` adds the attack flag, so only a dead unit that can be
    /// attacked is a candidate; a dead friend and a living foe are not.
    #[test]
    fn the_enemy_corpse_word_takes_the_dead_that_can_be_attacked() {
        assert!(admits(CORPSE_ENEMY, &foe(0)), "a dead foe");
        assert!(!admits(CORPSE_ENEMY, &friend(0)), "a dead friend");
        assert!(!admits(CORPSE_ENEMY, &foe(100)), "a living foe");
    }

    /// The health field is signed and an absent descriptor is dead, as the bind reads it.
    #[test]
    fn the_alive_test_is_the_binds() {
        let negative = friend(0x8000_0000);
        assert!(admits(CORPSE_ALLY, &negative), "signed health below 0");
        assert!(!admits(ASSIST_WORD, &negative));
        let empty = ObjectStore(ObjectFields::default());
        let factions = fx::factions();
        let me = fx::caster(1.5);
        let rel = TargetRelations {
            target_store: Some(&empty),
            target_owner_store: None,
            self_store: Some(&me),
            factions: Some(&factions),
            reputations: &Reputations(Vec::new()),
            types: Default::default(),
            group: Default::default(),
        };
        assert!(!PickFlags::of(UNIT).admits(false, &rel, || false));
        assert!(PickFlags::of(UNIT | DEAD_ONLY).admits(false, &rel, || true));
    }

    /// `480737`: while targeting, `UNIT_FIELD_FLAGS & 0x100` is refused, whatever the word.
    #[test]
    fn a_unit_immune_to_players_is_never_a_candidate() {
        // The word, and the subject it takes: (friend or foe, health).
        type Make = fn(u32, u32) -> ObjectStore;
        let cases: [(u16, Make, u32); 5] = [
            (UNIT, friend_with, 100),
            (ASSIST_WORD, friend_with, 100),
            (ENEMY, foe_with, 100),
            (CORPSE_ALLY, friend_with, 0),
            (UNIT | DEAD_ONLY, foe_with, 0),
        ];
        for (word, make, health) in cases {
            assert!(admits(word, &make(health, 0)), "{word:#06x}: the control");
            assert!(
                !admits(word, &make(health, UNIT_FLAG_IMMUNE_TO_PLAYER)),
                "{word:#06x}: flagged"
            );
        }
        // Only bit 8: another bit of the same nibble does not.
        let other = unit(&[(TEMPLATE, 1), (FLAGS, 0x0f8)]);
        assert!(admits(UNIT, &other));
    }

    /// The party leg is asked only when its flag is set, and one relation suffices.
    #[test]
    fn one_relation_of_those_asked_is_enough() {
        assert!(admits_as(PARTY_WORD, false, &foe(100), || true));
        assert!(!admits_as(PARTY_WORD, false, &foe(100), || false));
        // The assist relation does not stand in for the party flag alone.
        assert!(!admits_as(PARTY_WORD, false, &friend(100), || false));
        // A word with both flags: either passes.
        assert!(admits_as(
            PARTY_WORD | ASSIST_WORD,
            false,
            &friend(100),
            || false
        ));
        assert!(admits_as(
            PARTY_WORD | ASSIST_WORD,
            false,
            &foe(100),
            || true
        ));
        assert!(!admits_as(
            PARTY_WORD | ASSIST_WORD,
            false,
            &foe(100),
            || false
        ));
        // No party flag, no question asked.
        assert!(admits_as(
            ASSIST_WORD,
            false,
            &friend(100),
            || unreachable!()
        ));
    }

    /// Ourselves: the caster assists himself and never attacks himself, so a heal word takes him
    /// and an enemy word does not.
    #[test]
    fn the_caster_is_his_own_assist_and_never_his_own_attack() {
        assert!(admits_as(ASSIST_WORD, true, &friend(100), || false));
        assert!(!admits_as(ENEMY, true, &friend(100), || false));
    }

    /// The party leg's inputs, standing in a world: us (guid 1) and whoever a case spawns.
    fn party_world() -> (World, Entity) {
        let mut world = World::new();
        world.init_resource::<UnitPick>();
        world.init_resource::<GroupRoster>();
        world.init_resource::<SpellTargeting>();
        world.insert_resource(fx::factions());
        world.insert_resource(Reputations(Vec::new()));
        world.init_resource::<GuidIndex>();
        world.init_resource::<crate::ui_party::GroupState>();
        world.insert_resource(SelfGuid(Some(1)));
        let me = world
            .spawn((
                crate::net::SelfPlayer,
                Guid(1),
                GlobalTransform::default(),
                unit(&[(TYPE, PLAYER_TYPE), (TEMPLATE, 1), (FLAGS, 0x8)]),
            ))
            .id();
        world.resource_mut::<GuidIndex>().0.insert(1, me);
        (world, me)
    }

    fn spawn(world: &mut World, guid: u64, store: ObjectStore) -> Entity {
        let entity = world
            .spawn((Guid(guid), GlobalTransform::default(), store))
            .id();
        world.resource_mut::<GuidIndex>().0.insert(guid, entity);
        entity
    }

    fn seat(world: &mut World, guid: u64) {
        world
            .resource_mut::<crate::ui_party::GroupState>()
            .members
            .push(GroupMemberEntry {
                name: String::new(),
                guid,
                status: 0,
                flags: 0,
            });
    }

    /// The pick under `word` and the roster as it stands, published as a frame does.
    fn asked(world: &mut World, word: u16, entity: Entity) -> bool {
        world.resource_mut::<SpellTargeting>().enter(
            fx::RESURRECTION,
            crate::spell::CastCommit::Spell,
            word,
        );
        world
            .run_system_once(publish_unit_pick)
            .expect("the word publishes");
        world
            .run_system_once(publish_group_roster)
            .expect("the roster publishes");
        world
            .run_system_once(move |checks: PickChecks, stores: Query<&ObjectStore>| {
                checks.admits_unit(entity, stores.get(entity).ok())
            })
            .expect("the checks run")
    }

    fn player(extra: &[(u16, u32)]) -> ObjectStore {
        let mut pairs = vec![(TYPE, PLAYER_TYPE), (TEMPLATE, 1), (FLAGS, 0x8)];
        pairs.extend_from_slice(extra);
        unit(&pairs)
    }

    /// `0x606c20`, through the party word: us, a member of our party, our own pet and a member's
    /// pet pass; a stranger, a hostile creature, and a member who is not player-controlled do not.
    #[test]
    fn the_party_word_takes_us_and_the_people_of_our_party() {
        let (mut world, me) = party_world();
        let member = spawn(&mut world, 2, player(&[]));
        let stranger = spawn(&mut world, 3, player(&[]));
        let pet = spawn(&mut world, 4, unit(&[(FLAGS, 0x8), (CREATED_BY, 1)]));
        let member_pet = spawn(&mut world, 5, unit(&[(FLAGS, 0x8), (CHARMED_BY, 2)]));
        let stranger_pet = spawn(&mut world, 6, unit(&[(FLAGS, 0x8), (CREATED_BY, 3)]));
        let npc = spawn(&mut world, 7, foe(100));
        seat(&mut world, 2);

        assert!(asked(&mut world, PARTY_WORD, me), "ourselves (606c3d)");
        assert!(asked(&mut world, PARTY_WORD, member), "a party member");
        assert!(asked(&mut world, PARTY_WORD, pet), "our own pet");
        assert!(asked(&mut world, PARTY_WORD, member_pet), "a member's pet");
        assert!(
            !asked(&mut world, PARTY_WORD, stranger),
            "a player outside the party"
        );
        assert!(!asked(&mut world, PARTY_WORD, stranger_pet), "his pet");
        assert!(!asked(&mut world, PARTY_WORD, npc), "a creature");

        // Not player-controlled on either side: no party relation (606c5e, 606c76).
        world.entity_mut(member).insert(player(&[(FLAGS, 0)]));
        assert!(
            !asked(&mut world, PARTY_WORD, member),
            "a member without flag 0x8"
        );
        // A pet whose owner has not streamed answers no controller (606c92).
        let orphan = spawn(&mut world, 8, unit(&[(FLAGS, 0x8), (CREATED_BY, 99)]));
        assert!(!asked(&mut world, PARTY_WORD, orphan));
        // The raid word asks the same predicate.
        seat(&mut world, 3);
        assert!(asked(&mut world, RAID, stranger), "seated now");
        // Nobody is anybody's party once the roster is empty.
        world.init_resource::<crate::ui_party::GroupState>();
        world
            .resource_mut::<crate::ui_party::GroupState>()
            .members
            .clear();
        assert!(!asked(&mut world, PARTY_WORD, stranger));
        assert!(
            asked(&mut world, PARTY_WORD, me),
            "but we are still ourselves"
        );
    }
}
