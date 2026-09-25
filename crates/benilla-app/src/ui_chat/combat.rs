//! The combat log's chat lines, the presentation half of the reference's `UnitCombatLog_C`
//! (`~0x625000-0x62e7xx`): classify both endpoints, pick a chat type (`0x5e` drops the line), pick
//! a GlobalString key and fill it as `vsnprintf` does. The key resolves against the install's own
//! `GlobalStrings.lua` (`0x703bf0`); only the slot order is ours, one [`Slot`] list per family.

use bevy::prelude::*;

use crate::names::NameCache;
use crate::ui_chat::event::ChatEventKind;

use crate::net::{GuidIndex, NetCommands, ObjectStore, Reputations, SelfGuid};
use crate::target::ring::Factions;
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

impl Stores for Query<'_, '_, (Entity, &ObjectStore)> {
    fn store(&self, entity: Entity) -> Option<&ObjectStore> {
        self.get(entity).ok().map(|(_, s)| s)
    }
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

/// Classify one guid against the active player, the reference's `0x5efea0`. This reads the owner
/// (`UNIT_FIELD_CHARMEDBY`, then `UNIT_FIELD_CREATEDBY`) for every unit, tests party (your own
/// subgroup) before hostility, and runs `CanAttack` (`0x606980`) only with the player attacking
/// ([`crate::target::ring::can_attack_from_player`]). The reference reads the owner only for a
/// unit that is not a player (`0x5f0006`-`0x5f0019`); a player is hostile only if each can attack
/// the other (`0x5eff85`-`0x5effad`), and that test comes before party (`0x5effcd`).
pub(crate) fn classify(
    guid: u64,
    self_guid: &SelfGuid,
    group: Option<&GroupState>,
    index: &GuidIndex,
    stores: &impl Stores,
    factions: Option<&Factions>,
    reputations: &Reputations,
) -> UnitClass {
    let Some(me) = self_guid.0 else {
        return UnitClass::Unknown;
    };
    if guid == me {
        return UnitClass::Me;
    }
    let Some(entity) = index.0.get(&guid).copied() else {
        return UnitClass::Unknown;
    };
    let Some(store) = stores.store(entity) else {
        return UnitClass::Unknown;
    };
    // CHARMEDBY first, then CREATEDBY (`0x5f000c`/`0x5f0019`): an owned unit classifies by owner.
    let owner = store
        .0
        .unit_charmed_by()
        .or_else(|| store.0.unit_created_by());
    let owned_by = |g: u64| owner == Some(g);

    if owned_by(me) {
        return UnitClass::MyPet;
    }
    // The party slots hold your own subgroup only; `flags` bits 0-2 are the subgroup on both sides.
    let in_party = |g: u64| {
        group.is_some_and(|s| {
            s.in_group
                && s.members
                    .iter()
                    .any(|m| m.guid == g && m.flags & 0x7 == s.own_flags & 0x7)
        })
    };
    if in_party(guid) {
        return UnitClass::Party;
    }
    if owner.is_some_and(in_party) {
        return UnitClass::PartyPet;
    }

    // Friend or foe is `CanAttack` on this unit itself, never its owner (`0x5f00e3`/`0x5f00f2`).
    let hostile = {
        let me_store = index.0.get(&me).copied().and_then(|e| stores.store(e));
        crate::target::ring::can_attack_from_player(
            factions,
            reputations,
            Some(store),
            me_store,
            benilla_protocol::guid::is_player(guid),
        )
    };

    if benilla_protocol::guid::is_player(guid) {
        return if hostile {
            UnitClass::HostilePlayer
        } else {
            UnitClass::FriendlyPlayer
        };
    }
    if owner.is_some_and(benilla_protocol::guid::is_player) {
        return if hostile {
            UnitClass::HostilePet
        } else {
            UnitClass::FriendlyPet
        };
    }
    UnitClass::Creature
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
    // Resolved before the walk so the borrows outlive it.
    let school = fills.school.and_then(|s| school_word(lua, s));
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

/// `vsnprintf` over the `%s`/`%d` subset the combat-log templates use; `None` on any mismatch.
fn fill(template: &str, args: &[Arg<'_>]) -> Option<String> {
    let mut out = String::with_capacity(template.len() + 32);
    let mut next = args.iter();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            Some('s') => match next.next()? {
                Arg::Str(s) => out.push_str(s),
                Arg::Num(_) => return None,
            },
            Some('d') => match next.next()? {
                Arg::Num(n) => out.push_str(&n.to_string()),
                Arg::Str(_) => return None,
            },
            // Any other conversion is a mismatch, never a half-filled sentence.
            _ => return None,
        }
    }
    next.next().is_none().then_some(out)
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

/// One endpoint's display name, the reference's `GetObjectName` (`0x6264e0`); `None` until the
/// name cache answers, and the caller retries as the deferred queue at `0xc4e208` does. A streamed
/// unit names through its descriptor (`GetUnitName`, `0x609210`), which is how a pet is named.
pub(crate) fn object_name(
    guid: u64,
    unit: Option<&ObjectStore>,
    names: &NameCache,
    commands: &NetCommands,
) -> Option<String> {
    // Guid 0 means the name is already in the fills; no wire endpoint is guid 0.
    if guid == 0 {
        return None;
    }
    names.resolve_unit(guid, unit, commands).map(str::to_owned)
}

// ────────────────────────────────── the families ──────────────────────────────────────

/// Declare a four-way family: its key stem and its slots in the order the enUS template consumes
/// them, with that template and its `GlobalStrings.lua` line as the doc.
macro_rules! family {
    ($name:ident, $stem:literal, [$($slot:ident),* $(,)?], $doc:literal) => {
        #[doc = $doc]
        pub(crate) const $name: Family = Family {
            stem: $stem,
            keying: Keying::Quad,
            tail: "",
            slots: &[$(Slot::$slot),*],
        };
    };
}

/// Declare a two-way family, keyed on the subject alone, with an optional `tail`.
macro_rules! duo {
    ($name:ident, $stem:literal, $me:literal, $other:literal, [$($slot:ident),* $(,)?], $doc:literal) => {
        duo!($name, $stem, $me, $other, tail "", [$($slot),*], $doc);
    };
    ($name:ident, $stem:literal, $me:literal, $other:literal, tail $tail:literal, [$($slot:ident),* $(,)?], $doc:literal) => {
        #[doc = $doc]
        pub(crate) const $name: Family = Family {
            stem: $stem,
            keying: Keying::Duo { me: $me, other: $other },
            tail: $tail,
            slots: &[$(Slot::$slot),*],
        };
    };
}

/// Declare a family with no variant: the key is the whole name.
macro_rules! single {
    ($name:ident, $key:literal, [$($slot:ident),* $(,)?], $doc:literal) => {
        #[doc = $doc]
        pub(crate) const $name: Family = Family {
            stem: $key,
            keying: Keying::Single,
            tail: "",
            slots: &[$(Slot::$slot),*],
        };
    };
}

// ── melee: the swing that landed ───────────────────────────────────────────────────────
family!(
    COMBATHIT,
    "COMBATHIT",
    [Attacker, Victim, Amount],
    "`\"%s hits %s for %d.\"` (GlobalStrings.lua:777)"
);
family!(
    COMBATHITCRIT,
    "COMBATHITCRIT",
    [Attacker, Victim, Amount],
    "`\"%s crits %s for %d.\"` (:771)"
);
family!(
    COMBATHITSCHOOL,
    "COMBATHITSCHOOL",
    [Attacker, Victim, Amount, School],
    "`\"%s hits %s for %d %s damage.\"` (:779)"
);
family!(
    COMBATHITCRITSCHOOL,
    "COMBATHITCRITSCHOOL",
    [Attacker, Victim, Amount, School],
    "`\"%s crits %s for %d %s damage.\"` (:773)"
);

// ── melee: the swing that did not ──────────────────────────────────────────────────────
family!(
    MISSED,
    "MISSED",
    [Attacker, Victim],
    "`\"%s misses %s.\"` (:2698)"
);
family!(
    VSDODGE,
    "VSDODGE",
    [Attacker, Victim],
    "`\"%s attacks. %s dodges.\"` (:5400)"
);
family!(
    VSPARRY,
    "VSPARRY",
    [Attacker, Victim],
    "`\"%s attacks. %s parries.\"` (:5421)"
);
family!(
    VSBLOCK,
    "VSBLOCK",
    [Attacker, Victim],
    "`\"%s attacks. %s blocks.\"` (:5394)"
);
family!(
    VSABSORB,
    "VSABSORB",
    [Attacker, Victim],
    "`\"%s attacks. %s absorbs all the damage.\"` (:5391)"
);
family!(
    VSRESIST,
    "VSRESIST",
    [Attacker, Victim],
    "`\"%s attacks. %s resists all the damage.\"` (:5424)"
);
family!(
    VSIMMUNE,
    "VSIMMUNE",
    [Attacker, Victim],
    "`\"%s attacks but %s is immune.\"` (:5418)"
);
family!(
    VSEVADE,
    "VSEVADE",
    [Attacker, Victim],
    "`\"%s attacks. %s evades.\"` (:5415)"
);
family!(
    VSDEFLECT,
    "VSDEFLECT",
    [Attacker, Victim],
    "`\"%s attacks. %s deflects.\"` (:5397)"
);

// ── a spell's direct damage ────────────────────────────────────────────────────────────
family!(
    SPELLLOG,
    "SPELLLOG",
    [Attacker, Spell, Victim, Amount],
    "`\"%s's %s hits %s for %d.\"` (:3878)"
);
family!(
    SPELLLOGCRIT,
    "SPELLLOGCRIT",
    [Attacker, Spell, Victim, Amount],
    "`\"%s's %s crits %s for %d.\"` (:3870)"
);
family!(
    SPELLLOGSCHOOL,
    "SPELLLOGSCHOOL",
    [Attacker, Spell, Victim, Amount, School],
    "`\"%s's %s hits %s for %d %s damage.\"` (:3880)"
);
family!(
    SPELLLOGCRITSCHOOL,
    "SPELLLOGCRITSCHOOL",
    [Attacker, Spell, Victim, Amount, School],
    "`\"%s's %s crits %s for %d %s damage.\"` (:3872)"
);

// ── a spell that did not land ──────────────────────────────────────────────────────────
family!(
    SPELLMISS,
    "SPELLMISS",
    [Attacker, Spell, Victim],
    "`\"%s's %s missed %s.\"` (:3886)"
);
family!(
    SPELLRESIST,
    "SPELLRESIST",
    [Attacker, Spell, Victim],
    "`\"%s's %s was resisted by %s.\"` (:3911)"
);
family!(
    SPELLDODGED,
    "SPELLDODGED",
    [Attacker, Spell, Victim],
    "`\"%s's %s was dodged by %s.\"` (:3835)"
);
family!(
    SPELLPARRIED,
    "SPELLPARRIED",
    [Attacker, Spell, Victim],
    "`\"%s's %s was parried by %s.\"` (:3890)"
);
family!(
    SPELLBLOCKED,
    "SPELLBLOCKED",
    [Attacker, Spell, Victim],
    "`\"%s's %s was blocked by %s.\"` (:3817)"
);
family!(
    SPELLEVADED,
    "SPELLEVADED",
    [Attacker, Spell, Victim],
    "`\"%s's %s was evaded by %s.\"` (:3845)"
);
family!(
    SPELLIMMUNE,
    "SPELLIMMUNE",
    [Attacker, Spell, Victim],
    "`\"%s's %s fails. %s is immune.\"` (:3859)"
);
family!(
    SPELLDEFLECTED,
    "SPELLDEFLECTED",
    [Attacker, Spell, Victim],
    "`\"%s's %s was deflected by %s.\"` (:3829)"
);
family!(
    SPELLLOGABSORB,
    "SPELLLOGABSORB",
    [Attacker, Spell, Victim],
    "`\"%s's %s is absorbed by %s.\"` (:3866)"
);
family!(
    SPELLREFLECT,
    "SPELLREFLECT",
    [Attacker, Spell, Victim],
    "`\"%s's %s is reflected back by %s.\"` (:3907)"
);

// ── heals and power ────────────────────────────────────────────────────────────────────
family!(
    HEALED,
    "HEALED",
    [Attacker, Spell, Victim, Amount],
    "`\"%s's %s heals %s for %d.\"` (:2129)"
);
family!(
    HEALEDCRIT,
    "HEALEDCRIT",
    [Attacker, Spell, Victim, Amount],
    "`\"%s's %s critically heals %s for %d.\"` (:2125)"
);
family!(
    POWERGAIN,
    "POWERGAIN",
    [Victim, Amount, Power, Attacker, Spell],
    "`\"%s gains %d %s from %s's %s.\"` (:3091)"
);
family!(SPELLPOWERLEECH, "SPELLPOWERLEECH",
    [Attacker, Spell, Amount, Power, Victim, Attacker, Amount2, Power2],
    "`\"%s's %s drains %d %s from %s. %s gains %d %s.\"` (:3904) — the only family that spells the \
     subject TWICE, and the second one is the same name as the first (the drainer is the gainer).");

// ── periodic ticks ─────────────────────────────────────────────────────────────────────
family!(
    PERIODICAURADAMAGE,
    "PERIODICAURADAMAGE",
    [Victim, Amount, School, Attacker, Spell],
    "`\"%s suffers %d %s damage from %s's %s.\"` (:3001)"
);
family!(
    PERIODICAURAHEAL,
    "PERIODICAURAHEAL",
    [Victim, Amount, Attacker, Spell],
    "`\"%s gains %d health from %s's %s.\"` (:3005)"
);

// ── a damage shield firing back ────────────────────────────────────────────────────────
family!(
    DAMAGESHIELD,
    "DAMAGESHIELD",
    [Attacker, Amount, School, Victim],
    "`\"%s reflects %d %s damage to %s.\"` (:884) — the SUBJECT is the shield's BEARER and the \
     object is whoever struck them, which is the reverse of the packet's own field names."
);

// ── a unit dying ───────────────────────────────────────────────────────────────────────
duo!(
    UNITDIES,
    "UNITDIES",
    "SELF",
    "OTHER",
    [Attacker],
    "`\"You die.\"` (:4426) / `\"%s dies.\"` (:4425) — the death reflex's ordinary line. The SUBJECT \
     is the unit that died; there is no second endpoint."
);
single!(
    UNITDESTROYEDOTHER,
    "UNITDESTROYEDOTHER",
    [Attacker],
    "`\"%s is destroyed.\"` (:4424) — what a SUMMONED thing does instead of dying. There is no \
     `…SELF` twin: you are never a summon."
);
single!(
    SELFKILLOTHER,
    "SELFKILLOTHER",
    [Attacker],
    "`\"You have slain %s!\"` (:3408) — `SMSG_PARTYKILLLOG` when the killer is YOU. The reference \
     pushes (victim, killer) and the string consumes only the first, so the subject is the VICTIM."
);
single!(
    PARTYKILLOTHER,
    "PARTYKILLOTHER",
    [Attacker, Victim],
    "`\"%s is slain by %s!\"` (:2988) — the same packet when the killer is a party member. \
     (victim, killer), in that order."
);

// ── an aura landing, stacking, leaving, being dispelled or stolen ──────────────────────
duo!(
    AURAADDED_HARMFUL,
    "AURAADDED",
    "SELF",
    "OTHER",
    tail "HARMFUL",
    [Attacker, Spell],
    "`\"You are afflicted by %s.\"` (:102) / `\"%s is afflicted by %s.\"` (:100) — UNIT first."
);
duo!(
    AURAADDED_HELPFUL,
    "AURAADDED",
    "SELF",
    "OTHER",
    tail "HELPFUL",
    [Attacker, Spell],
    "`\"You gain %s.\"` (:103) / `\"%s gains %s.\"` (:101)"
);
duo!(
    AURAAPPLICATIONADDED_HARMFUL,
    "AURAAPPLICATIONADDED",
    "SELF",
    "OTHER",
    tail "HARMFUL",
    [Attacker, Spell, Amount],
    "`\"You are afflicted by %s (%d).\"` (:106) / `\"%s is afflicted by %s (%d).\"` (:104)"
);
duo!(
    AURAAPPLICATIONADDED_HELPFUL,
    "AURAAPPLICATIONADDED",
    "SELF",
    "OTHER",
    tail "HELPFUL",
    [Attacker, Spell, Amount],
    "`\"You gain %s (%d).\"` (:107) / `\"%s gains %s (%d).\"` (:105)"
);
duo!(
    AURAREMOVED,
    "AURAREMOVED",
    "SELF",
    "OTHER",
    [Spell, Attacker],
    "`\"%s fades from you.\"` (:113) / `\"%s fades from %s.\"` (:112) — AURA first, which is the \
     other way round from `AURAADDED*`. The flip is the reference's own (`0x62b480`) and is \
     exactly the kind of thing a single ordered slot list per family exists to pin."
);
duo!(
    AURADISPEL,
    "AURADISPEL",
    "SELF",
    "OTHER",
    [Attacker, Spell],
    "`\"Your %s is removed.\"` (:111) / `\"%s's %s is removed.\"` (:110) — the subject is the aura's \
     BEARER, not the dispeller, who the sentence never names."
);

// ── an enchant landing on or fading from an item ───────────────────────────────────────
family!(
    ITEMENCHANTMENTADD,
    "ITEMENCHANTMENTADD",
    [Attacker, Spell, Victim, Named],
    "`\"%s casts %s on %s's %s.\"` (:2378) — caster, enchant, owner, ITEM. The item name is always \
     last and always spelled."
);
duo!(
    ITEMENCHANTMENTREMOVE,
    "ITEMENCHANTMENTREMOVE",
    "SELF",
    "OTHER",
    [Spell, Attacker, Named],
    "`\"%s has faded from your %s.\"` (:2383) / `\"%s has faded from %s's %s.\"` (:2382) — enchant, \
     owner, item. A fade names no caster, which is why it drops to two keys."
);

// ── what a cast made, fed or opened ────────────────────────────────────────────────────
duo!(
    TRADESKILL_LOG,
    "TRADESKILL_LOG",
    "_FIRSTPERSON",
    "_THIRDPERSON",
    [Attacker, Named],
    "`\"You create %s.\"` (:4273) / `\"%s creates %s.\"` (:4274) — the one family pair whose two-way \
     words are not SELF/OTHER."
);
duo!(
    FEEDPET_LOG,
    "FEEDPET_LOG",
    "_FIRSTPERSON",
    "_THIRDPERSON",
    [Attacker, Named],
    "`\"Your pet begins eating the %s.\"` (:1965) / `\"%s's pet begins eating a %s.\"` (:1966)"
);

// ── the spell-outcome leaves ───────────────────────────────────────────────────────────
family!(
    PROCRESIST,
    "PROCRESIST",
    [Victim, Attacker, Spell],
    "`\"%s resists %s's %s.\"` (:3100) — TARGET first, then caster (the reference's convention B)."
);
family!(
    IMMUNESPELL,
    "IMMUNESPELL",
    [Victim, Attacker, Spell],
    "`\"%s is immune to %s's %s.\"` (:2317) — convention B."
);
family!(
    DISPELFAILED,
    "DISPELFAILED",
    [Attacker, Victim, Spell],
    "`\"%s fails to dispel %s's %s.\"` (:933) — convention C: caster, target, spell."
);
family!(
    SPELLINTERRUPT,
    "SPELLINTERRUPT",
    [Attacker, Victim, Spell],
    "`\"%s interrupts %s's %s.\"` (:3863) — convention C. No `…SELFSELF`."
);
duo!(
    INSTAKILL,
    "INSTAKILL",
    "SELF",
    "OTHER",
    [Attacker, Spell],
    "`\"You are killed by %s.\"` (:2330) / `\"%s is killed by %s.\"` (:2329) — subject is the unit \
     killed."
);
family!(
    SPELLSPLITDAMAGE,
    "SPELLSPLITDAMAGE",
    [Attacker, Spell, Victim, Amount],
    "`\"%s's %s causes %s %d damage.\"` (:3916) — the `hit_info & 8` split-damage form of \
     `SMSG_SPELLNONMELEEDAMAGELOG`. No `…SELFSELF`."
);
family!(
    SPELLPOWERDRAIN,
    "SPELLPOWERDRAIN",
    [Attacker, Spell, Amount, Power, Victim],
    "`\"%s's %s drains %d %s from %s.\"` (:3900) — [`SPELLPOWERLEECH`]'s twin, chosen when the \
     leech multiplier is effectively zero: the same push block, three slots shorter."
);
duo!(
    SPELLEXTRAATTACKS,
    "SPELLEXTRAATTACKS",
    "SELF",
    "OTHER",
    [Attacker, Amount, Spell],
    "`\"You gain %d extra attacks through %s.\"` (:3851) / `\"%s gains %d extra attacks through \
     %s.\"` (:3849)"
);
duo!(
    SPELLEXTRAATTACKS_SINGULAR,
    "SPELLEXTRAATTACKS",
    "SELF",
    "OTHER",
    tail "_SINGULAR",
    [Attacker, Amount, Spell],
    "`\"You gain %d extra attack through %s.\"` (:3852) — the reference `SStrCat`s `_SINGULAR` onto \
     the same key when the count is 1, so this is the same family with a tail rather than a fifth \
     name."
);
family!(
    SPELLDURABILITYDAMAGE,
    "SPELLDURABILITYDAMAGE",
    [Attacker, Spell, Victim, Named],
    "`\"%s casts %s on %s: %s damaged.\"` (:3842) — the trailing name is the ITEM. No `…SELFSELF`."
);
family!(
    SPELLDURABILITYDAMAGEALL,
    "SPELLDURABILITYDAMAGEALL",
    [Attacker, Spell, Victim],
    "`\"%s casts %s on %s: all items damaged.\"` (:3839) — what the same effect says when its slot \
     and item id are both `-1`. No `…SELFSELF`."
);
duo!(
    SPELLDISMISSPET,
    "SPELLDISMISSPET",
    "SELF",
    "OTHER",
    [Attacker, Named],
    "`\"Your %s is dismissed.\"` (:3834) / `\"%s's %s is dismissed.\"` (:3833) — the trailing name \
     is the PET."
);
duo!(
    SPELLHAPPINESSDRAIN,
    "SPELLHAPPINESSDRAIN",
    "SELF",
    "OTHER",
    [Attacker, Named, Amount],
    "`\"Your %s loses %d happiness.\"` (:3858) / `\"%s's %s loses %d happiness.\"` (:3857) — power \
     type 4 has no `…_POINTS` noun, so happiness gets its own family instead of a `POWERGAIN` row."
);

// ── a cast that failed ─────────────────────────────────────────────────────────────────
duo!(
    SPELLFAILCAST,
    "SPELLFAILCAST",
    "SELF",
    "OTHER",
    [Attacker, Spell, Named],
    "`\"You fail to cast %s: %s.\"` (:3854) / `\"%s fails to cast %s: %s.\"` (:3853) — the trailing \
     name is the REASON, already localized."
);
duo!(
    SPELLFAILPERFORM,
    "SPELLFAILPERFORM",
    "SELF",
    "OTHER",
    [Attacker, Spell, Named],
    "`\"You fail to perform %s: %s.\"` (:3856) / `\"%s fails to perform %s: %s.\"` (:3855)"
);

// ── the miscellaneous 0x19 leaves and the reputation line ──────────────────────────────
single!(
    DURABILITYDAMAGE_DEATH,
    "DURABILITYDAMAGE_DEATH",
    [],
    "`\"Your equipped items suffer a 10%% durability loss.\"` (:963) — `SMSG_DURABILITY_DAMAGE_DEATH` \
     carries an EMPTY body, so the line has no arguments at all."
);
single!(
    PET_LOYALTY_GAIN,
    "PET_LOYALTY_GAIN",
    [],
    "`\"Your pet's loyalty has increased.\"` (:3043). The reference emits it as `(\"%s\", GetText(key))`; \
     a no-slot family is the same sentence by a shorter road."
);
single!(
    PET_LOYALTY_LOSS,
    "PET_LOYALTY_LOSS",
    [],
    "`\"Your pet's loyalty has decreased.\"` (:3044)"
);
single!(
    FACTION_STANDING_INCREASED,
    "FACTION_STANDING_INCREASED",
    [Named, Amount],
    "`\"Your %s reputation has increased by %d.\"` (:1946) — the name is the FACTION's."
);
single!(
    FACTION_STANDING_DECREASED,
    "FACTION_STANDING_DECREASED",
    [Named, Amount],
    "`\"Your %s reputation has decreased by %d.\"` (:1945)"
);

// ── environmental damage ───────────────────────────────────────────────────────────────
/// Declare one `VSENVIRONMENTALDAMAGE_<TYPE>_{SELF,OTHER}` pair, the key the reference builds with
/// `snprintf` over the six type names at `0x80dcac`.
macro_rules! env_family {
    ($name:ident, $stem:literal, $doc:literal) => {
        duo!($name, $stem, "SELF", "OTHER", [Attacker, Amount], $doc);
    };
}
env_family!(
    VSENV_FATIGUE,
    "VSENVIRONMENTALDAMAGE_FATIGUE_",
    "`\"You are exhausted and lose %d health.\"` (:5408) — damage type 0."
);
env_family!(
    VSENV_DROWNING,
    "VSENVIRONMENTALDAMAGE_DROWNING_",
    "`\"You are drowning and lose %d health.\"` (:5404) — type 1."
);
env_family!(
    VSENV_FALLING,
    "VSENVIRONMENTALDAMAGE_FALLING_",
    "`\"You fall and lose %d health.\"` (:5406) — type 2."
);
env_family!(
    VSENV_LAVA,
    "VSENVIRONMENTALDAMAGE_LAVA_",
    "`\"You lose %d health for swimming in lava.\"` (:5412) — type 3."
);
env_family!(
    VSENV_SLIME,
    "VSENVIRONMENTALDAMAGE_SLIME_",
    "`\"You lose %d health for swimming in slime.\"` (:5414) — type 4."
);
env_family!(
    VSENV_FIRE,
    "VSENVIRONMENTALDAMAGE_FIRE_",
    "`\"You suffer %d points of fire damage.\"` (:5410) — type 5."
);

/// The environmental family for a `SMSG_ENVIRONMENTALDAMAGELOG` damage type; `None` past the six,
/// where the reference indexes its table unguarded (`0x62abc5`).
pub(crate) fn env_family(damage_type: u8) -> Option<Family> {
    Some(match damage_type {
        0 => VSENV_FATIGUE,
        1 => VSENV_DROWNING,
        2 => VSENV_FALLING,
        3 => VSENV_LAVA,
        4 => VSENV_SLIME,
        5 => VSENV_FIRE,
        _ => return None,
    })
}

/// Every family this module emits, for the sweep against the shipped templates; a family left out
/// goes unchecked.
#[cfg(test)]
pub(crate) const ALL_FAMILIES: &[Family] = &[
    COMBATHIT,
    COMBATHITCRIT,
    COMBATHITSCHOOL,
    COMBATHITCRITSCHOOL,
    MISSED,
    VSDODGE,
    VSPARRY,
    VSBLOCK,
    VSABSORB,
    VSRESIST,
    VSIMMUNE,
    VSEVADE,
    VSDEFLECT,
    SPELLLOG,
    SPELLLOGCRIT,
    SPELLLOGSCHOOL,
    SPELLLOGCRITSCHOOL,
    SPELLMISS,
    SPELLRESIST,
    SPELLDODGED,
    SPELLPARRIED,
    SPELLBLOCKED,
    SPELLEVADED,
    SPELLIMMUNE,
    SPELLDEFLECTED,
    SPELLLOGABSORB,
    SPELLREFLECT,
    HEALED,
    HEALEDCRIT,
    POWERGAIN,
    SPELLPOWERLEECH,
    PERIODICAURADAMAGE,
    PERIODICAURAHEAL,
    DAMAGESHIELD,
    UNITDIES,
    UNITDESTROYEDOTHER,
    SELFKILLOTHER,
    PARTYKILLOTHER,
    AURAADDED_HARMFUL,
    AURAADDED_HELPFUL,
    AURAAPPLICATIONADDED_HARMFUL,
    AURAAPPLICATIONADDED_HELPFUL,
    AURAREMOVED,
    AURADISPEL,
    ITEMENCHANTMENTADD,
    ITEMENCHANTMENTREMOVE,
    TRADESKILL_LOG,
    FEEDPET_LOG,
    PROCRESIST,
    IMMUNESPELL,
    DISPELFAILED,
    SPELLINTERRUPT,
    INSTAKILL,
    SPELLSPLITDAMAGE,
    SPELLPOWERDRAIN,
    SPELLEXTRAATTACKS,
    SPELLEXTRAATTACKS_SINGULAR,
    SPELLDURABILITYDAMAGE,
    SPELLDURABILITYDAMAGEALL,
    SPELLDISMISSPET,
    SPELLHAPPINESSDRAIN,
    SPELLFAILCAST,
    SPELLFAILPERFORM,
    DURABILITYDAMAGE_DEATH,
    PET_LOYALTY_GAIN,
    PET_LOYALTY_LOSS,
    FACTION_STANDING_INCREASED,
    FACTION_STANDING_DECREASED,
    VSENV_FATIGUE,
    VSENV_DROWNING,
    VSENV_FALLING,
    VSENV_LAVA,
    VSENV_SLIME,
    VSENV_FIRE,
];

/// The family that words a `SpellMissInfo` byte (vmangos `SpellDefines.h:160-174`), as the
/// reference's miss switch `0x62bb50` maps it.
pub(crate) fn miss_family(miss_info: u8) -> Family {
    match miss_info {
        2 => SPELLRESIST,
        3 => SPELLDODGED,
        4 => SPELLPARRIED,
        5 => SPELLBLOCKED,
        6 => SPELLEVADED,
        // 7 IMMUNE and 8 IMMUNE2 are one outcome with two server-side spellings.
        7 | 8 => SPELLIMMUNE,
        9 => SPELLDEFLECTED,
        11 => SPELLREFLECT,
        // 10 ABSORB, 0, 1 and anything out of range take the default arm `0x62c0e0`, not
        // `SPELLLOGABSORB`, which belongs to the direct-damage path.
        _ => SPELLMISS,
    }
}

/// The melee family a swing selects, the reference's dispatcher `0x629b60` arm for arm; the order
/// matters because the tests overlap. The bits are vmangos's `HitInfo` in its `> 1.9.4` branch
/// (`Objects/UnitDefines.h:250-268`) and `VictimState` (`:237-248`).
pub(crate) fn melee_family(
    hit_info: u32,
    victim_state: u32,
    damage: u32,
    school: u8,
) -> Option<Family> {
    const MISS: u32 = 0x10;
    const ABSORB: u32 = 0x20;
    const RESIST: u32 = 0x40;
    const CRIT: u32 = 0x80;
    if hit_info & MISS != 0 {
        return Some(MISSED);
    }
    if victim_state == 5 {
        return Some(VSBLOCK);
    }
    if damage == 0 {
        if hit_info & ABSORB != 0 {
            return Some(VSABSORB);
        }
        if hit_info & RESIST != 0 {
            return Some(VSRESIST);
        }
    }
    if victim_state == 1 && damage > 0 {
        return Some(match (hit_info & CRIT != 0, school != 0) {
            (false, false) => COMBATHIT,
            (true, false) => COMBATHITCRIT,
            (false, true) => COMBATHITSCHOOL,
            (true, true) => COMBATHITCRITSCHOOL,
        });
    }
    // `0x62a710` words only a state its flag table `0x8628f8` marks (`0x62a720`) and its jump
    // table `0x62a8ec` covers: 0, 1 without damage, 4, 9 and 10 or more print nothing.
    match victim_state {
        2 => Some(VSDODGE),
        3 => Some(VSPARRY),
        6 => Some(VSEVADE),
        7 => Some(VSIMMUNE),
        8 => Some(VSDEFLECT),
        _ => None,
    }
}

// ────────────────────────── the queued line, awaiting its names ───────────────────────

/// A combat-log line classified at the packet, with only its names outstanding: the reference's
/// `0x629b60` decides while both units are streamed and parks the message on the deferred queue
/// at `0xc4e208` until the name-ready callback `0x6294b0`, so a despawned creature still logs.
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
    })
}

pub(crate) mod watch;

#[cfg(test)]
mod tests;
