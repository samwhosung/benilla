//! The combat log's chat lines, the presentation half of the reference's `UnitCombatLog_C`
//! (`~0x625000-0x62e7xx`): classify both endpoints, pick a chat type (`0x5e` drops the line), pick
//! a GlobalString key and fill it as `vsnprintf` does. The key resolves against the install's own
//! `GlobalStrings.lua` (`0x703bf0`); only the slot order is ours, one [`Slot`] list per family.

use bevy::prelude::*;

use crate::names::NameCache;
use crate::ui_chat::event::ChatEventKind;

use crate::net::{GuidIndex, ObjectStore, Reputations, SelfGuid};
use crate::query_cache::Ask;
use crate::target::ring::Factions;
use crate::target::{RelUnit, Relations, UnitLookup};
use crate::ui_party::GroupState;

/// Read-only `ObjectStore` lookup by entity, the one thing [`classify`] needs from the world.
pub(crate) trait Stores {
    fn store(&self, entity: Entity) -> Option<&ObjectStore>;
}

impl Stores for Query<'_, '_, &mut ObjectStore> {
    fn store(&self, entity: Entity) -> Option<&ObjectStore> {
        self.get(entity).ok()
    }
}

impl Stores for Query<'_, '_, &ObjectStore> {
    fn store(&self, entity: Entity) -> Option<&ObjectStore> {
        self.get(entity).ok()
    }
}

impl Stores for Query<'_, '_, (Entity, &ObjectStore)> {
    fn store(&self, entity: Entity) -> Option<&ObjectStore> {
        self.get(entity).ok().map(|(_, s)| s)
    }
}

/// Whether the client holds `guid` as a unit, the `0x468460` lookup with type mask 8 that the
/// handlers make before a flash anchor, a float or a full block: a player or a creature, never a
/// game object.
pub(crate) fn holds_unit(guid: u64, index: &GuidIndex, stores: &impl Stores) -> bool {
    index
        .0
        .get(&guid)
        .and_then(|&e| stores.store(e))
        .is_some_and(is_unit)
}

/// Whether a held object is a unit, type bit 3 of `OBJECT_FIELD_TYPE`: a player or a creature.
pub(crate) fn is_unit(store: &ObjectStore) -> bool {
    use benilla_protocol::messages::ObjectType;
    matches!(
        store.0.object_type(),
        Some(ObjectType::Unit | ObjectType::Player)
    )
}

/// A world position by entity, for the range gate.
pub(crate) trait Poses {
    fn pose(&self, entity: Entity) -> Option<Vec3>;
}

impl Poses for Query<'_, '_, &mut Transform> {
    fn pose(&self, entity: Entity) -> Option<Vec3> {
        self.get(entity).ok().map(|t| t.translation)
    }
}

impl Poses for Query<'_, '_, &Transform> {
    fn pose(&self, entity: Entity) -> Option<Vec3> {
        self.get(entity).ok().map(|t| t.translation)
    }
}

/// One endpoint's half of the reference's display-range gate (`0x626630`, ranges from `0x626810`):
/// the squared 3-D distance from the active player, strictly below `range` squared, so the
/// boundary and a NaN are out. `100000.0` always passes and `0.0` always refuses; a pose not yet
/// held counts as in range, so a despawning mob's killing blow is not lost.
pub(crate) fn in_range(
    guid: u64,
    range: f32,
    self_guid: &SelfGuid,
    index: &GuidIndex,
    poses: &impl Poses,
) -> bool {
    if range >= 100_000.0 {
        return true;
    }
    if range <= 0.0 {
        return false;
    }
    let pose = |g: u64| index.0.get(&g).copied().and_then(|e| poses.pose(e));
    let (Some(me), Some(them)) = (self_guid.0.and_then(pose), pose(guid)) else {
        return true;
    };
    me.distance_squared(them) < range * range
}

/// The combat log's live display ranges in yards: the seven class CVars of the reference's table
/// at `0x8629e0` plus `CombatDeathLogRange`, registered at `0x626d00`. The gate reads the CVar's
/// float field (`+0x24`); `0` silences a class, so nothing clamps.
#[derive(Resource, Clone, Copy)]
pub(crate) struct CombatLogRanges {
    /// By [`UnitClass`] index; classes 0, 1 and 9 have no CVar and keep their sentinels.
    class: [f32; 10],
    /// `CombatDeathLogRange`, the death line's own range, outside the `0x8629e0` table.
    death: f32,
}

impl Default for CombatLogRanges {
    fn default() -> Self {
        let mut class = [0.0; 10];
        for (i, slot) in class.iter_mut().enumerate() {
            *slot = UnitClass::from_index(i).default_range();
        }
        Self {
            class,
            death: DEATH_LOG_RANGE_DEFAULT,
        }
    }
}

impl CombatLogRanges {
    /// This class's live display range, in yards.
    pub(crate) fn class(&self, class: UnitClass) -> f32 {
        self.class[class as usize]
    }

    /// `CombatDeathLogRange`, for every class alike: the death formatter `0x62c160` reads the CVar
    /// (`0x62c19c`) and falls back to the class range only when the lookup fails, which a
    /// registered CVar never does.
    pub(crate) fn death(&self) -> f32 {
        self.death
    }

    /// Whether `name` is one of the eight; asked before [`Self::set`] so another CVar does not mark
    /// the resource changed.
    pub(crate) fn is_range_cvar(&self, name: &str) -> bool {
        name.eq_ignore_ascii_case(DEATH_LOG_RANGE_CVAR)
            || (0..self.class.len()).any(|i| {
                UnitClass::from_index(i)
                    .range_cvar()
                    .is_some_and(|c| c.eq_ignore_ascii_case(name))
            })
    }

    /// Apply one `SetCVar`; `true` if the name was one of the eight.
    pub(crate) fn set(&mut self, name: &str, value: f32) -> bool {
        if name.eq_ignore_ascii_case(DEATH_LOG_RANGE_CVAR) {
            self.death = value;
            return true;
        }
        for i in 0..self.class.len() {
            let class = UnitClass::from_index(i);
            if class
                .range_cvar()
                .is_some_and(|c| c.eq_ignore_ascii_case(name))
            {
                self.class[i] = value;
                return true;
            }
        }
        false
    }
}

/// The combat log CVars' change callback: the eight display ranges and the periodic switch.
pub(crate) fn on_cvar(
    ev: On<crate::cvars::CvarChanged>,
    mut ranges: ResMut<CombatLogRanges>,
    mut periodic: ResMut<LogPeriodicSpells>,
) {
    if ev.is(LOG_PERIODIC_CVAR) {
        periodic.0 = ev.flag();
    } else if ranges.is_range_cvar(&ev.name) {
        ranges.set(&ev.name, ev.num());
    }
}

/// `CombatLogPeriodicSpells`. Off, the `SMSG_PERIODICAURALOG` handler `0x626dd0` returns at its
/// first read (`0x626dee`), dropping the chat lines, the floating tick number and the miss word;
/// it also filters the periodic `SMSG_SPELLNONMELEEDAMAGELOG` line (`0x62d9ae`) and the periodic
/// `IMMUNESPELL*` lines (`0x62d25f`). Read as the CVar's int field (`+0x28`, set at `0x63e127`).
#[derive(Resource, Clone, Copy)]
pub(crate) struct LogPeriodicSpells(pub(crate) bool);

impl Default for LogPeriodicSpells {
    /// The reference's registered default, `"1"`.
    fn default() -> Self {
        Self(true)
    }
}

pub(crate) const LOG_PERIODIC_CVAR: &str = "CombatLogPeriodicSpells";

/// `CombatDeathLogRange`, registered at `0x626d5f` with default `"60"` (`0x862e14`).
pub(crate) const DEATH_LOG_RANGE_CVAR: &str = "CombatDeathLogRange";
pub(crate) const DEATH_LOG_RANGE_DEFAULT: f32 = 60.0;

/// A unit's standing relative to the active player: the `0..9` index every combat-log selector
/// takes, in the order of the reference's range table at `0x8629e0`, which `0x626810` indexes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum UnitClass {
    Me = 0,
    MyPet = 1,
    Party = 2,
    PartyPet = 3,
    FriendlyPlayer = 4,
    FriendlyPet = 5,
    HostilePlayer = 6,
    HostilePet = 7,
    Creature = 8,
    /// Unresolvable: not streamed, or a guid with no unit behind it.
    Unknown = 9,
}

impl UnitClass {
    pub(crate) fn from_index(i: usize) -> Self {
        match i {
            0 => Self::Me,
            1 => Self::MyPet,
            2 => Self::Party,
            3 => Self::PartyPet,
            4 => Self::FriendlyPlayer,
            5 => Self::FriendlyPet,
            6 => Self::HostilePlayer,
            7 => Self::HostilePet,
            8 => Self::Creature,
            _ => Self::Unknown,
        }
    }

    /// The CVar naming this class's display range; classes 0, 1 and 9 have none.
    pub(crate) fn range_cvar(self) -> Option<&'static str> {
        Some(match self {
            Self::Me | Self::MyPet | Self::Unknown => return None,
            Self::Party => "CombatLogRangeParty",
            Self::PartyPet => "CombatLogRangePartyPet",
            Self::FriendlyPlayer => "CombatLogRangeFriendlyPlayers",
            Self::FriendlyPet => "CombatLogRangeFriendlyPlayersPets",
            Self::HostilePlayer => "CombatLogRangeHostilePlayers",
            Self::HostilePet => "CombatLogRangeHostilePlayersPets",
            Self::Creature => "CombatLogRangeCreature",
        })
    }

    /// The registered default in yards, from the `0x8629e0` pairs: the sentinels are `100000.0`
    /// (`[0x80dcc4]`) for classes 0 and 1 and `0.0` (`[0x7ffd74]`) for 9.
    pub(crate) fn default_range(self) -> f32 {
        match self {
            Self::Me | Self::MyPet => 100_000.0,
            Self::Creature => 30.0,
            Self::Unknown => 0.0,
            _ => 50.0,
        }
    }

    /// The `SELF` of a `…SELFOTHER`/`…OTHERSELF` key: class 0 only. Your pet is an `OTHER`, told
    /// apart by its chat type (`COMBAT_PET_HITS`).
    fn is_me(self) -> bool {
        self == Self::Me
    }
}

/// The object manager as [`classify`] asks it: a guid's descriptor through the index, and a unit
/// read with its creature record, as [`crate::target::RelationWorld::unit`] reads one.
struct Objects<'a, S: Stores> {
    index: &'a GuidIndex,
    stores: &'a S,
    names: Option<&'a NameCache>,
}

impl<S: Stores> Objects<'_, S> {
    fn store(&self, guid: u64) -> Option<&ObjectStore> {
        self.stores.store(*self.index.0.get(&guid)?)
    }
}

impl<S: Stores> UnitLookup for Objects<'_, S> {
    fn unit(&self, guid: u64) -> Option<RelUnit> {
        let store = self.store(guid).filter(|s| s.is_unit())?;
        let unit = RelUnit::of(guid, store);
        Some(match self.names {
            Some(names) => unit.with_record(store, names),
            None => unit,
        })
    }
}

/// `ClassifyGuidForPlayer 0x5efea0`, one guid's standing to the active player. Both the active
/// player and the guid's object must be held (`0x5efefc`, `0x5eff04`); a game object stands for
/// the unit that created it (`GAMEOBJECT_CREATED_BY`, `0x5eff1e`-`0x5eff45`).
/// - The active player is 0 (`0x5eff5c`).
/// - A player is hostile, 6, when it and the active player can each attack the other
///   (`0x5eff85`-`0x5effad`), before any party test; else a party member, 2 (`0x5effcd`); else
///   friendly, 4. A game object's creator takes the class one up (`0x5effa1`-`0x5effad`).
/// - Any other unit goes by its charmer, else its creator (`0x5f0006`-`0x5f0019`), never a
///   player's: none is a creature, 8; the active player 1; a party member 3; any other held
///   player 7 when the unit itself and the active player can each attack the other
///   (`0x5f00e3`-`0x5f00fd`), else 5; an owner that is no held player, 8.
///
/// [`Relations::can_attack`] is `CanAttack 0x606980` with its argument order kept: the unit
/// attacking the player is asked first.
pub(crate) fn classify(
    guid: u64,
    self_guid: &SelfGuid,
    group: Option<&GroupState>,
    index: &GuidIndex,
    stores: &impl Stores,
    factions: Option<&Factions>,
    reputations: &Reputations,
    names: Option<&NameCache>,
) -> UnitClass {
    let objects = Objects {
        index,
        stores,
        names,
    };
    let roster = group.map(crate::spell::group_relation::GroupRoster::party_of);
    let rel = Relations {
        factions,
        reputations,
        me: self_guid.0,
        roster: roster.as_ref(),
        units: &objects,
    };
    classify_in(&rel, guid, |g| objects.store(g))
}

/// [`classify`] over the relation inputs and a guid's descriptor of any object type.
fn classify_in<'a>(
    rel: &Relations,
    guid: u64,
    object: impl Fn(u64) -> Option<&'a ObjectStore>,
) -> UnitClass {
    use benilla_protocol::messages::ObjectType;
    use UnitClass as C;
    let (Some(me), Some(store)) = (rel.me.and_then(|g| rel.units.unit(g)), object(guid)) else {
        return C::Unknown;
    };
    let (unit, from_object) = match store.0.object_type() {
        Some(ObjectType::Unit | ObjectType::Player) => (rel.units.unit(guid), false),
        Some(ObjectType::GameObject) => (
            store
                .0
                .gameobject_created_by()
                .and_then(|g| rel.units.unit(g)),
            true,
        ),
        _ => (None, false),
    };
    let Some(unit) = unit else {
        return C::Unknown;
    };
    let up = |class: C| {
        if from_object {
            C::from_index(class as usize + 1)
        } else {
            class
        }
    };
    if unit.guid == me.guid {
        return up(C::Me);
    }
    let mutual = |unit: &RelUnit| rel.can_attack(unit, &me) && rel.can_attack(&me, unit);
    if unit.is_player {
        return up(if mutual(&unit) {
            C::HostilePlayer
        } else if rel.in_party(unit.guid) {
            C::Party
        } else {
            C::FriendlyPlayer
        });
    }
    let owner = if unit.charmed_by != 0 {
        unit.charmed_by
    } else {
        unit.created_by
    };
    if owner == 0 {
        return C::Creature;
    }
    if owner == me.guid {
        return C::MyPet;
    }
    if rel.in_party(owner) {
        return C::PartyPet;
    }
    match rel.units.unit(owner) {
        Some(o) if o.is_player && mutual(&unit) => C::HostilePet,
        Some(o) if o.is_player => C::FriendlyPet,
        _ => C::Creature,
    }
}

/// `SPELL_ATTR_DO_NOT_DISPLAY | SPELL_ATTR_DO_NOT_LOG`, `Attributes` (`SpellRec+0x18`) bits 7 and
/// 8 (vmangos `SpellDefines.h:799-800`).
const ATTR_DO_NOT_DISPLAY_OR_LOG: u32 = 0x180;

/// The spell a combat-log formatter names, or `None` where it writes no line: each of the 24
/// formatters that index the Spell store (`0xc0d788`), `0x626a10` to `0x62e190`, returns for an
/// id out of range or with no record, for `Attributes & 0x180` and for an empty name in the
/// client's locale, as the miss line's `0x62bac3`-`0x62bb07` does.
pub(crate) fn logged_spell(
    spells: Option<&crate::ui_action::Spells>,
    spell_id: u32,
) -> Option<&benilla_formats::SpellDisplay> {
    let display = spells?.catalog.get(spell_id)?;
    (display.attributes & ATTR_DO_NOT_DISPLAY_OR_LOG == 0 && !display.name.is_empty())
        .then_some(display)
}

/// `SPELL_ATTR_DO_NOT_LOG`, `Attributes` bit 8 (vmangos `SpellDefines.h:800`).
const ATTR_DO_NOT_LOG: u32 = 0x100;

/// The spell record a packet's `UNIT_COMBAT` and floating text need: the handlers test for a
/// record without `Attributes & 0x100` before either (`0x5e87e3`-`0x5e87f4`, `0x626f1b`,
/// `0x5e7f0b`-`0x5e7f18`), apart from the line's own test, [`logged_spell`].
pub(crate) fn feedback_spell(
    spells: Option<&crate::ui_action::Spells>,
    spell_id: u32,
) -> Option<&benilla_formats::SpellDisplay> {
    spells?
        .catalog
        .get(spell_id)
        .filter(|d| d.attributes & ATTR_DO_NOT_LOG == 0)
}

// ─────────────────────────────── the msgType selectors ────────────────────────────────

/// The melee `(attacker, victim)` chat type, the reference's selector pair `0x62a0d0`/`0x62a2e0`;
/// a miss is the row after the hit. A party or friendly player hitting you, your pet or your party
/// types as HOSTILEPLAYER, the duel case, without consulting faction; their pets do not.
pub(crate) fn combat_kind(
    attacker: UnitClass,
    victim: UnitClass,
    miss: bool,
) -> Option<ChatEventKind> {
    use ChatEventKind as K;
    use UnitClass as C;
    let mine = matches!(victim, C::Me | C::MyPet | C::Party | C::PartyPet);
    let hits = match attacker {
        C::Me => K::CombatSelfHits,
        C::MyPet => K::CombatPetHits,
        C::Party if mine => K::CombatHostilePlayerHits,
        C::Party | C::PartyPet => K::CombatPartyHits,
        C::FriendlyPlayer if mine => K::CombatHostilePlayerHits,
        C::FriendlyPlayer | C::FriendlyPet => K::CombatFriendlyPlayerHits,
        C::HostilePlayer | C::HostilePet => K::CombatHostilePlayerHits,
        C::Creature | C::Unknown => match victim {
            C::Me | C::MyPet => K::CombatCreatureVsSelfHits,
            C::Party | C::PartyPet => K::CombatCreatureVsPartyHits,
            _ => K::CombatCreatureVsCreatureHits,
        },
    };
    Some(if miss { miss_twin(hits) } else { hits })
}

/// The direct-spell `(attacker, victim)` chat type (`0x627820` and its damage sibling), shaped as
/// [`combat_kind`]: heals, power gains and auras are BUFF, damage and failures are DAMAGE.
pub(crate) fn spell_kind(
    attacker: UnitClass,
    victim: UnitClass,
    buff: bool,
) -> Option<ChatEventKind> {
    use ChatEventKind as K;
    use UnitClass as C;
    let mine = matches!(victim, C::Me | C::MyPet | C::Party | C::PartyPet);
    let damage = match attacker {
        C::Me => K::SpellSelfDamage,
        C::MyPet => K::SpellPetDamage,
        C::Party if mine => K::SpellHostilePlayerDamage,
        C::Party | C::PartyPet => K::SpellPartyDamage,
        C::FriendlyPlayer if mine => K::SpellHostilePlayerDamage,
        C::FriendlyPlayer | C::FriendlyPet => K::SpellFriendlyPlayerDamage,
        C::HostilePlayer | C::HostilePet => K::SpellHostilePlayerDamage,
        C::Creature | C::Unknown => match victim {
            C::Me | C::MyPet => K::SpellCreatureVsSelfDamage,
            C::Party | C::PartyPet => K::SpellCreatureVsPartyDamage,
            _ => K::SpellCreatureVsCreatureDamage,
        },
    };
    Some(if buff { buff_twin(damage) } else { damage })
}

/// The periodic chat type (`0x627d80` damage, `0x6274a0` buffs), off one class, with no PET row and
/// no `CREATURE_VS_*` split. The class is the target for the `PERIODICAURADAMAGE*` and
/// `PERIODICAURAHEAL*` formatters (`0x628100`, `0x627240`) and the caster for `POWERGAIN*` and
/// `SPELLPOWERLEECH*`/`…DRAIN*` (`0x627520`, `0x627930`).
pub(crate) fn periodic_kind(subject: UnitClass, buff: bool) -> Option<ChatEventKind> {
    use ChatEventKind as K;
    use UnitClass as C;
    let damage = match subject {
        C::Me | C::MyPet => K::SpellPeriodicSelfDamage,
        C::Party | C::PartyPet => K::SpellPeriodicPartyDamage,
        C::FriendlyPlayer | C::FriendlyPet => K::SpellPeriodicFriendlyPlayerDamage,
        C::HostilePlayer | C::HostilePet => K::SpellPeriodicHostilePlayerDamage,
        C::Creature | C::Unknown => K::SpellPeriodicCreatureDamage,
    };
    Some(if buff { buff_twin(damage) } else { damage })
}

/// The death chat type (`0x628980`), off the victim's class: 0 to 5 friendly, the rest hostile.
pub(crate) fn death_kind(victim: UnitClass) -> ChatEventKind {
    if (victim as u8) <= 5 {
        ChatEventKind::CombatFriendlyDeath
    } else {
        ChatEventKind::CombatHostileDeath
    }
}

/// The aura-fade chat type (`0x62b7d0`), off the bearer's class. An aura landing types through
/// [`periodic_kind`] instead (`0x62b480`).
pub(crate) fn aura_gone_kind(bearer: UnitClass) -> ChatEventKind {
    use ChatEventKind as K;
    use UnitClass as C;
    match bearer {
        C::Me | C::MyPet => K::SpellAuraGoneSelf,
        C::Party | C::PartyPet => K::SpellAuraGoneParty,
        _ => K::SpellAuraGoneOther,
    }
}

/// The damage-shield chat type (`0x62c140`), off one class. Every `SMSG_SPELLLOGMISS` line types
/// through it too (`0x5e7f31` routes `0x62bab0` here), not through the spell matrix.
pub(crate) fn damage_shield_kind(subject: UnitClass) -> ChatEventKind {
    if matches!(subject, UnitClass::Me | UnitClass::MyPet) {
        ChatEventKind::SpellDamageShieldsOnSelf
    } else {
        ChatEventKind::SpellDamageShieldsOnOthers
    }
}

/// HITS to MISSES, the next row of the reference's chat type table.
fn miss_twin(hits: ChatEventKind) -> ChatEventKind {
    use ChatEventKind as K;
    match hits {
        K::CombatSelfHits => K::CombatSelfMisses,
        K::CombatPetHits => K::CombatPetMisses,
        K::CombatPartyHits => K::CombatPartyMisses,
        K::CombatFriendlyPlayerHits => K::CombatFriendlyPlayerMisses,
        K::CombatHostilePlayerHits => K::CombatHostilePlayerMisses,
        K::CombatCreatureVsSelfHits => K::CombatCreatureVsSelfMisses,
        K::CombatCreatureVsPartyHits => K::CombatCreatureVsPartyMisses,
        K::CombatCreatureVsCreatureHits => K::CombatCreatureVsCreatureMisses,
        other => other,
    }
}

/// DAMAGE to BUFF, the next row of the chat type table.
fn buff_twin(damage: ChatEventKind) -> ChatEventKind {
    use ChatEventKind as K;
    match damage {
        K::SpellSelfDamage => K::SpellSelfBuff,
        K::SpellPetDamage => K::SpellPetBuff,
        K::SpellPartyDamage => K::SpellPartyBuff,
        K::SpellFriendlyPlayerDamage => K::SpellFriendlyPlayerBuff,
        K::SpellHostilePlayerDamage => K::SpellHostilePlayerBuff,
        K::SpellCreatureVsSelfDamage => K::SpellCreatureVsSelfBuff,
        K::SpellCreatureVsPartyDamage => K::SpellCreatureVsPartyBuff,
        K::SpellCreatureVsCreatureDamage => K::SpellCreatureVsCreatureBuff,
        K::SpellPeriodicSelfDamage => K::SpellPeriodicSelfBuffs,
        K::SpellPeriodicPartyDamage => K::SpellPeriodicPartyBuffs,
        K::SpellPeriodicFriendlyPlayerDamage => K::SpellPeriodicFriendlyPlayerBuffs,
        K::SpellPeriodicHostilePlayerDamage => K::SpellPeriodicHostilePlayerBuffs,
        K::SpellPeriodicCreatureDamage => K::SpellPeriodicCreatureBuffs,
        other => other,
    }
}

// ──────────────────────────── the format-string selectors ─────────────────────────────

/// Which of a family's four templates applies: the variant code the reference's 45 selectors
/// (`0x629f90`, `0x62a290`, …) return. `SelfSelf` prints nothing unless the family has the key,
/// as `SPELLLOGSELFSELF` and `HEALEDSELFSELF` do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Variant {
    /// I did it to someone else: `…SELFOTHER`, variant code 2.
    SelfOther,
    /// Someone else did it to me: `…OTHERSELF`, code 1.
    OtherSelf,
    /// Someone else, to someone else: `…OTHEROTHER`, code 3.
    OtherOther,
    /// Me, to me: `…SELFSELF`, code 0 where the family has no such key.
    SelfSelf,
}

impl Variant {
    fn of(subject: UnitClass, object: UnitClass) -> Self {
        match (subject.is_me(), object.is_me()) {
            (true, false) => Self::SelfOther,
            (false, true) => Self::OtherSelf,
            (false, false) => Self::OtherOther,
            (true, true) => Self::SelfSelf,
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Self::SelfOther => "SELFOTHER",
            Self::OtherSelf => "OTHERSELF",
            Self::OtherOther => "OTHEROTHER",
            Self::SelfSelf => "SELFSELF",
        }
    }

    /// Whether the subject is spelled by name rather than as "You".
    fn names_subject(self) -> bool {
        matches!(self, Self::OtherSelf | Self::OtherOther)
    }

    fn names_object(self) -> bool {
        matches!(self, Self::SelfOther | Self::OtherOther)
    }

    /// Whether the subject is the local player, the one bit a [`Keying::Duo`] family keys on.
    fn subject_is_me(self) -> bool {
        matches!(self, Self::SelfSelf | Self::SelfOther)
    }
}

/// How a family builds its GlobalString key from the stem and the variant: the four-way selectors
/// (`0x62a290` and 44 more), a two-way off a sentence's single participant (`0x62c160`,
/// `0x62b480`, `0x629610`), or one fixed key (`DURABILITYDAMAGE_DEATH`, `SELFKILLOTHER`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Keying {
    /// `…SELFSELF` / `…SELFOTHER` / `…OTHERSELF` / `…OTHEROTHER`, off both endpoints.
    Quad,
    /// Two keys off the subject alone, in the family's words: `SELF`/`OTHER`, or
    /// `_FIRSTPERSON`/`_THIRDPERSON` for `TRADESKILL_LOG` and `FEEDPET_LOG`.
    Duo {
        me: &'static str,
        other: &'static str,
    },
    /// One key: the stem and tail are the whole name.
    Single,
}

/// One value slot of a family's format string, in the order the shipped template consumes it.
/// `Attacker` and `Victim` appear only in the variants that spell that endpoint by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Slot {
    /// `%s`: the subject's name, when the variant spells it.
    Attacker,
    /// `%s`: the object's name, when the variant spells it.
    Victim,
    /// `%s`: the spell's name.
    Spell,
    /// `%s`: the damage school's word (`SPELL_SCHOOL<n>_CAP`).
    School,
    /// `%s`: a power's word (`MANA_POINTS` and kin).
    Power,
    /// `%d`: the primary amount.
    Amount,
    /// `%d`: a second amount, the leech family's gain.
    Amount2,
    /// `%s`: the leech family's gained power word.
    Power2,
    /// `%s`: a name that is never "you" (an item, gameobject, pet, faction or failure reason),
    /// spelled in every variant.
    Named,
}

/// A combat-log message family: the stem its keys are built from, and the ordered slots its
/// templates consume.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Family {
    pub stem: &'static str,
    pub keying: Keying,
    /// Appended after the variant word, as `AURAADDED` + `SELF` + `HARMFUL`; empty for most.
    pub tail: &'static str,
    pub slots: &'static [Slot],
}

impl Family {
    /// The GlobalString key this family resolves for `variant`.
    pub(crate) fn key(&self, variant: Variant) -> String {
        let mid = match self.keying {
            Keying::Quad => variant.suffix(),
            Keying::Duo { me, other } => {
                if variant.subject_is_me() {
                    me
                } else {
                    other
                }
            }
            Keying::Single => "",
        };
        format!("{}{mid}{}", self.stem, self.tail)
    }

    fn names_subject(&self, variant: Variant) -> bool {
        match self.keying {
            Keying::Quad => variant.names_subject(),
            // Only the `…OTHER` half of a two-way key spells a name.
            Keying::Duo { .. } => !variant.subject_is_me(),
            // A fixed key such as `PARTYKILLOTHER` always names its subject.
            Keying::Single => true,
        }
    }

    fn names_object(&self, variant: Variant) -> bool {
        match self.keying {
            Keying::Quad => variant.names_object(),
            // A two-way family's sentence has one participant, so no `Victim` slot.
            Keying::Duo { .. } => false,
            Keying::Single => true,
        }
    }
}

/// The six suffixes `0x628410` appends to a finished sentence, in its order, before `0x626850`
/// emits the whole line as one `%s`; a suffix whose GlobalString is empty is skipped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Trailers {
    pub absorbed: u32,
    /// Negative is a vulnerability bonus, worded by `VULNERABLE_TRAILER`.
    pub resisted: i32,
    pub blocked: u32,
    /// The swing's `HitInfo`, for `0x4000` GLANCING and `0x8000` CRUSHING; melee only.
    pub hit_info: u32,
}

fn append_trailers(lua: &benilla_ui::script::UiScript, line: &mut String, t: Trailers) {
    const GLANCING: u32 = 0x4000;
    const CRUSHING: u32 = 0x8000;
    let mut push = |key: &str, arg: Option<i64>| {
        let Some(template) = global_string(lua, key) else {
            return;
        };
        let args = match arg {
            Some(n) => vec![Arg::Num(n)],
            None => Vec::new(),
        };
        match fill(&template, &args) {
            Some(text) => line.push_str(&text),
            None => warn!("combat log: {key} does not match its trailer shape"),
        }
    };
    if t.hit_info & GLANCING != 0 {
        push("GLANCING_TRAILER", None);
    }
    if t.hit_info & CRUSHING != 0 {
        push("CRUSHING_TRAILER", None);
    }
    if t.resisted > 0 {
        push("RESIST_TRAILER", Some(i64::from(t.resisted)));
    } else if t.resisted < 0 {
        push("VULNERABLE_TRAILER", Some(-i64::from(t.resisted)));
    }
    if t.blocked != 0 {
        push("BLOCK_TRAILER", Some(i64::from(t.blocked)));
    }
    if t.absorbed != 0 {
        push("ABSORB_TRAILER", Some(i64::from(t.absorbed)));
    }
}

/// The value bound to each [`Slot`] for one line. School and power are indices, resolved to words
/// in [`compose_line`]; `None` where the family needs one drops the line.
#[derive(Clone, Debug, Default)]
pub(crate) struct Fills {
    pub attacker: String,
    pub victim: String,
    pub spell: String,
    /// The damage school index, 0 physical to 6 arcane.
    pub school: Option<u8>,
    /// The vmangos `Powers` index, 0 mana to 4 happiness.
    pub power: Option<u32>,
    pub amount: i64,
    pub amount2: i64,
    /// The leech family's gained power index.
    pub power2: Option<u32>,
    /// The [`Slot::Named`] text, already localized.
    pub named: String,
    /// The `0x628410` suffixes, for the families that can grow one.
    pub trailers: Option<Trailers>,
}

/// Resolve a family and variant to a finished line; `None`, silently, when the key is absent,
/// where the reference prints a not-found warning line (`0x6269f0`) for any key but a missing
/// `…SELFSELF`. The fill is `vsnprintf` over a fixed argument list, as at `0x626850`.
pub(crate) fn compose_line(
    lua: &benilla_ui::script::UiScript,
    family: Family,
    variant: Variant,
    fills: &Fills,
) -> Option<String> {
    let key = family.key(variant);
    let template = global_string(lua, &key)?;
    // Resolved before the walk so the borrows outlive it. A school with no name writes no line,
    // whatever the template takes: each formatter that names a school returns first on it
    // (`0x62cd99`, `0x628117`, `0x629d50`, `0x62ca3e`).
    let school = match fills.school {
        Some(s) => Some(school_word(lua, s)?),
        None => None,
    };
    let power = fills.power.and_then(|p| power_word(lua, p));
    let power2 = fills.power2.and_then(|p| power_word(lua, p));
    let mut args: Vec<Arg> = Vec::with_capacity(family.slots.len());
    for slot in family.slots {
        match slot {
            Slot::Attacker if family.names_subject(variant) => args.push(Arg::Str(&fills.attacker)),
            Slot::Victim if family.names_object(variant) => args.push(Arg::Str(&fills.victim)),
            Slot::Attacker | Slot::Victim => {}
            Slot::Named => args.push(Arg::Str(&fills.named)),
            Slot::Spell => args.push(Arg::Str(&fills.spell)),
            Slot::School => args.push(Arg::Str(school.as_deref()?)),
            Slot::Power => args.push(Arg::Str(power.as_deref()?)),
            Slot::Power2 => args.push(Arg::Str(power2.as_deref()?)),
            Slot::Amount => args.push(Arg::Num(fills.amount)),
            Slot::Amount2 => args.push(Arg::Num(fills.amount2)),
        }
    }
    match fill(&template, &args) {
        Some(mut line) => {
            if let Some(t) = fills.trailers {
                append_trailers(lua, &mut line, t);
            }
            Some(line)
        }
        None => {
            // Our slot order disagrees with the shipped template: a defect, so it warns.
            warn!(
                "combat log: {key} does not match our slot order ({:?})",
                family.slots
            );
            None
        }
    }
}

/// One `vsnprintf` argument.
enum Arg<'a> {
    Str(&'a str),
    Num(i64),
}

/// The line's formatter over the `%s`/`%d` subset the combat-log templates use. `0x626850` formats
/// through `SStrVPrintf 0x64a8b0`, which takes Storm's ordered printf (`0x6697d0`) once option
/// `0xa` is set, as the client does at start (`0x402141`, `0x64ede9`); that printf reads `%N$`
/// (`0x669af2`), so a numbered hole (`%2$d`, the deDE templates') names its argument in the list
/// enUS fills in sequence (`Ihr trefft %1$s. Schaden: %2$d.`). `None` on any mismatch, an unused
/// argument included.
fn fill(template: &str, args: &[Arg<'_>]) -> Option<String> {
    let mut out = String::with_capacity(template.len() + 32);
    let mut used = vec![false; args.len()];
    let mut cursor = 0;
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        if chars.next_if_eq(&'%').is_some() {
            out.push('%');
            continue;
        }
        let mut number = String::new();
        while let Some(d) = chars.next_if(char::is_ascii_digit) {
            number.push(d);
        }
        let index = if number.is_empty() {
            cursor += 1;
            cursor - 1
        } else {
            chars.next_if_eq(&'$')?;
            number.parse::<usize>().ok()?.checked_sub(1)?
        };
        let arg = args.get(index)?;
        used[index] = true;
        match (chars.next(), arg) {
            (Some('s'), Arg::Str(s)) => out.push_str(s),
            (Some('d'), Arg::Num(n)) => out.push_str(&n.to_string()),
            // Any other conversion is a mismatch, never a half-filled sentence.
            _ => return None,
        }
    }
    used.iter().all(|&u| u).then_some(out)
}

/// A GlobalString off the VM's globals, `None` when absent or empty: the two tests `0x703bf0`'s
/// callers make (`GetPVPRankInfo` at `0x51aa1c`/`0x51aa20`).
pub(crate) fn global_string(script: &benilla_ui::script::UiScript, key: &str) -> Option<String> {
    script
        .lua()
        .globals()
        .get::<Option<String>>(key)
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
}

/// The capitalized school word a `…SCHOOL…` template takes. The reference reads it from
/// `Resistances.dbc` (`0x6264b0`, rows at `[0xc0d9a4]`); this reads `SPELL_SCHOOL<n>_CAP`, the
/// same seven words in enUS, where the lowercase `SPELL_SCHOOL<n>_NAME` would be wrong.
pub(crate) fn school_word(script: &benilla_ui::script::UiScript, school: u8) -> Option<String> {
    global_string(script, &format!("SPELL_SCHOOL{school}_CAP"))
}

/// The GlobalString key `0x6278f0` resolves for a power tag: the five-entry table at `0x85645c`,
/// happiness included (`GlobalStrings.lua:2117`), and NULL for 5 or more by an unsigned compare.
fn power_key(power: u32) -> Option<&'static str> {
    match power {
        0 => Some("MANA_POINTS"),
        1 => Some("RAGE_POINTS"),
        2 => Some("FOCUS_POINTS"),
        3 => Some("ENERGY_POINTS"),
        4 => Some("HAPPINESS_POINTS"),
        _ => None,
    }
}

/// Whether `0x6278f0` answers a noun at all, askable without a VM: the formatters gate on it
/// before any template (`0x627964`).
pub(crate) fn power_has_word(power: u32) -> bool {
    power_key(power).is_some()
}

/// The power word a `POWERGAIN`/`SPELLPOWERLEECH`/`SPELLPOWERDRAIN` template takes, by the wire's
/// `Powers` index.
pub(crate) fn power_word(script: &benilla_ui::script::UiScript, power: u32) -> Option<String> {
    global_string(script, power_key(power)?)
}

/// One endpoint's display name as `GetObjectName 0x6264e0` finds it: what the caches hold now, a
/// lookup asked for what they do not (`0x6265a7`, `0x6265e0`, with an empty callback), and `None`
/// for that ([`logged_name`] prints the miss). A streamed unit names through its descriptor
/// (`GetUnitName`, `0x609210`), which is how a pet is named.
pub(crate) fn object_name(
    guid: u64,
    unit: Option<&ObjectStore>,
    names: &NameCache,
    ask: Ask<'_>,
) -> Option<String> {
    // Guid 0 means the name is already in the fills; no wire endpoint is guid 0.
    if guid == 0 {
        return None;
    }
    names.resolve_unit(guid, unit, ask).map(str::to_owned)
}

/// The name `GetObjectName` prints for an object nothing names: the `UNKNOWNOBJECT` string, else
/// the literal `UKNOWNOBJECT` (`0x6265f4`-`0x62660c`).
pub(crate) fn unknown_object(script: &benilla_ui::script::UiScript) -> String {
    global_string(script, "UNKNOWNOBJECT").unwrap_or_else(|| "UKNOWNOBJECT".into())
}

/// One endpoint's name as `GetObjectName 0x6264e0` prints it: [`object_name`], else the miss of
/// the getter it went through. A unit the client holds (type bit 3, `0x626526`) goes through
/// `GetUnitName` (`0x626531`), whose miss is `UNKNOWNOBJECT` else the literal "Unknown Being"
/// (`0x609324`-`0x60933c`); anything else misses as [`unknown_object`].
pub(crate) fn logged_name(
    guid: u64,
    store: Option<&ObjectStore>,
    names: &NameCache,
    ask: Ask<'_>,
    script: &benilla_ui::script::UiScript,
) -> String {
    object_name(guid, store, names, ask).unwrap_or_else(|| {
        if store.is_some_and(is_unit) {
            script.unknown_object_text()
        } else {
            unknown_object(script)
        }
    })
}

// ────────────────────────── the queued line, awaiting its names ───────────────────────

/// A combat-log line classified at the packet, composed at the drain with the names the caches
/// hold then. The reference's deferred queue (`0xc4e208`, replayed by `0x6294b0`) holds only a
/// line waiting on an item name (`0x55ba30`); a unit's name never holds one ([`Self::fill_names`]).
#[derive(Clone, Debug)]
pub(crate) struct PendingCombat {
    pub kind: ChatEventKind,
    pub family: Family,
    pub variant: Variant,
    /// The sentence's subject, naming the `Attacker` slots; for [`DAMAGESHIELD`], the bearer.
    pub subject: u64,
    /// The guid naming the `Victim` slots.
    pub object: u64,
    /// Everything the template needs but the names, which are filled at drain.
    pub fills: Fills,
    pub named: Named,
    pub tries: u16,
    /// The `COMBAT_TEXT_UPDATE` the formatter raises once the line is written, and only then: a
    /// missing template, range or spell writes neither.
    pub word: Option<LineWord>,
}

/// One `COMBAT_TEXT_UPDATE` a line raises: a type name of the table `0x862920`, then `arg2` and
/// `arg3` as the helpers `0x626890`-`0x6269d0` format them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LineWord {
    pub message_type: &'static str,
    pub data: WordText,
    pub extra: Option<String>,
}

impl LineWord {
    /// The type alone, as `0x6269d0` raises it.
    pub(crate) fn bare(message_type: &'static str) -> Self {
        Self {
            message_type,
            data: WordText::Text(None),
            extra: None,
        }
    }

    /// The type and one argument: a number (`0x626960`) or a name (`0x6269b0`).
    pub(crate) fn with(message_type: &'static str, data: impl ToString) -> Self {
        Self {
            message_type,
            data: WordText::Text(Some(data.to_string())),
            extra: None,
        }
    }

    /// The type, the name the line gives its subject, and a number (`0x626890`).
    pub(crate) fn named(message_type: &'static str, extra: impl ToString) -> Self {
        Self {
            message_type,
            data: WordText::SubjectName,
            extra: Some(extra.to_string()),
        }
    }

    /// The type and two numbers (`0x6268f0`).
    pub(crate) fn pair(
        message_type: &'static str,
        data: impl ToString,
        extra: impl ToString,
    ) -> Self {
        Self {
            message_type,
            data: WordText::Text(Some(data.to_string())),
            extra: Some(extra.to_string()),
        }
    }
}

/// A word's `arg2`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WordText {
    /// This text, or nil.
    Text(Option<String>),
    /// The name the line gives its subject, the healer's or the caster's `GetObjectName`
    /// (`0x62c8ea`, `0x627414`), filled with the line's own names.
    SubjectName,
}

impl PendingCombat {
    /// The word's `arg2` once the names are in.
    pub(crate) fn word_data(&self, word: &LineWord) -> Option<String> {
        match &word.data {
            WordText::Text(text) => text.clone(),
            WordText::SubjectName => Some(self.fills.attacker.clone()),
        }
    }
}

impl PendingCombat {
    /// Fill the names as `GetObjectName 0x6264e0` names an object: a unit by what `name` prints
    /// now ([`logged_name`]), never waiting. Only an item the item cache has not answered holds the
    /// line (`0x628f40`, `0x629610`, `0x629760`, `0x6298a0` waiting through `0x55ba30`); `false`
    /// while it holds. A guid of 0 is a name the fills already carry.
    pub(crate) fn fill_names(
        &mut self,
        name: impl Fn(u64) -> String,
        item: impl FnOnce(u32) -> ItemName,
    ) -> bool {
        if self.subject != 0 {
            self.fills.attacker = name(self.subject);
        }
        if self.object != 0 {
            self.fills.victim = name(self.object);
        }
        match self.named {
            Named::Ready => true,
            Named::Unit(guid) => {
                self.fills.named = name(guid);
                true
            }
            Named::Item(entry) => match item(entry) {
                ItemName::Named(text) => {
                    self.fills.named = text;
                    true
                }
                // The server answered the item unknown: the line goes out with what the arm left.
                ItemName::Unknown => true,
                ItemName::Waiting => false,
            },
        }
    }
}

/// What the item cache answers a line waiting on an item's name.
pub(crate) enum ItemName {
    Named(String),
    /// The server answered that it has no such item.
    Unknown,
    /// No answer yet: the line keeps waiting.
    Waiting,
}

/// What still has to be looked up before a line's [`Slot::Named`] can be filled: an item through
/// the item cache (`0x55ba30`), a unit through `GetObjectName` (`0x6264e0`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Named {
    /// The text is already in [`Fills::named`], or the family has no `Named` slot.
    Ready,
    /// An item entry, through the ask-once item cache.
    Item(u32),
    /// A unit or object guid, through the name cache.
    Unit(u64),
}

/// Build a queued line from an already-classified pair; `None` when neither endpoint resolves.
pub(crate) fn queue(
    kind: ChatEventKind,
    family: Family,
    subject: (u64, UnitClass),
    object: (u64, UnitClass),
    fills: Fills,
    named: Named,
) -> Option<PendingCombat> {
    // The range gate is an OR over the two endpoints (`0x626630`): one unresolvable endpoint alone
    // does not drop the line.
    if subject.1 == UnitClass::Unknown && object.1 == UnitClass::Unknown {
        return None;
    }
    Some(PendingCombat {
        kind,
        family,
        variant: Variant::of(subject.1, object.1),
        subject: subject.0,
        object: object.0,
        fills,
        named,
        tries: 0,
        word: None,
    })
}

mod families;
pub(crate) use families::*;
mod outcome;
pub(crate) use outcome::*;

pub(crate) mod watch;

#[cfg(test)]
mod tests;
