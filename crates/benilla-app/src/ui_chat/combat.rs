//! The combat log's chat lines — the content pipeline decision 0288 §3 named out of its own scope
//! ("transcribing ATTACKERSTATEUPDATE/spell-log packets into COMBATHIT strings is the combat-log
//! arc"), and B297 is the bill for leaving it. The wire has been decoded for a long time; it fed
//! the floating numbers, the portrait indicator and the center text, and produced **no chat at
//! all**, which is why a damage meter or Quiver's TranqAnnouncer sees nothing to parse.
//!
//! This module is the reference's `UnitCombatLog_C` TU (`~0x625000-0x62e7xx`), the presentation
//! half: it owns **no** arithmetic, only the
//! composition — classify both endpoints, pick a chat type, pick a GlobalString *key*, fill it.
//! The reference's own chain is four hops and we take the same four:
//!
//! ```text
//!   packet  →  classify(attacker), classify(victim)   →  0..9  (the range table names them)
//!           →  msgType selector                       →  the chat type  (0x5e = drop)
//!           →  format-string selector                 →  a GlobalString KEY + a variant code
//!           →  FrameScript_GetText(key) → vsnprintf    →  the line
//! ```
//!
//! **We ship keys, never Blizzard's text.** The reference's selectors (`0x62a290`, `0x629f90`, the
//! other 43) return the *name* of a localized template; `0x703bf0` (`FrameScript_GetText`) resolves
//! it, and when it comes back empty `0x6269f0` prints "Warning: string %s not found" instead of a
//! line. benilla already runs the player's own `Interface\FrameXML\GlobalStrings.lua` into the VM
//! at boot ([`crate::ui_script`]'s `load_global_strings`), so [`global_string`] is that same hop:
//! the key is ours, the text is the install's. That is the licence-clean shape *and* the faithful
//! one — and it is why a non-enUS install gets its own language for free.
//!
//! **The slot orders are the one thing we author.** A family's format string is Blizzard's, but
//! which value lands in which `%s`/`%d` is a fact about that string, and we state it as an ordered
//! [`Slot`] list. Two properties make that safe rather than fragile: within a family the order is
//! **invariant across the four variants** (the variant only decides whether the `Attacker`/`Victim`
//! slots are *present* — "Your %s hits %s for %d." and "%s's %s hits %s for %d." are the same list
//! with one slot dropped), and [`tests`] fills every declared family against the *real*
//! `GlobalStrings.lua` and checks the `%s`/`%d` type signature it produces matches the shipped
//! template's. A transposition cannot survive that; a drift cannot either.

use bevy::prelude::*;

use crate::names::NameCache;
use crate::ui_chat::event::ChatEventKind;

use crate::net::{GuidIndex, NetCommands, ObjectStore, Reputations, SelfGuid};
use crate::target::ring::Factions;
use crate::ui_party::GroupState;

/// Read-only `ObjectStore` lookup by entity — the only thing [`classify`] needs from the world.
///
/// It exists because the two callers hold different query shapes and neither is wrong: the net
/// drain already has a `&mut` store query in hand (whose `get` is read-only anyway), while the
/// watcher systems in [`watch`] hold the `(Entity, &ObjectStore)` pair they sweep. One trait beats
/// either duplicating the classifier or forcing a query shape on a system that does not want it.
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

/// [`Stores`]' twin for the range gate: a world position by entity.
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

/// One endpoint's half of the reference's display-range gate (`0x626630` → the per-class range
/// getter `0x626810`): a 3-D squared distance from the ACTIVE PLAYER to the endpoint, against the
/// class's range squared.
///
/// **The comparison is strictly `<`** — `fcomp; fnstsw ax; test ah,5; jnp`, so `dist² == range²` is
/// OUT and a NaN is out. 1571 used `<=`; the boundary is exact in the binary and there is no reason
/// for us to be looser.
///
/// **The range is the caller's, read off [`CombatLogRanges`]** — the live table the reference's
/// `0x626810` walks, not a compiled-in constant. The caller picks which entry: a class range for
/// the fourteen two-ended and one-ended formatters, and [`CombatLogRanges::death`] for the one
/// formatter that has a range CVar of its own. The two sentinels ride in the table like any other
/// entry: `100000.0` makes the gate a no-op for you and your pet, `0.0` refuses an unresolvable
/// unit outright.
///
/// A pose we do not hold is treated as **in** range: dropping a line because a unit's transform had
/// not landed yet would silently lose the killing blow on a mob that despawns, which is a worse
/// failure than logging one fight too far away.
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

/// **The combat log's display ranges, live** — the reference's `{cvarName, defaultValue}` table at
/// `0x8629e0` plus the one range CVar that sits outside it, `CombatDeathLogRange`.
///
/// The reference registers all eight in one place (`0x626d00`, a loop over `0x8629e0` skipping the
/// NULL/empty names, then one unrolled call), stores **no
/// handle for any of them**, and looks each up by name at every use. We keep the resolved numbers
/// instead: the lookup-by-name is the reference's way of not caching, not a behaviour, and
/// `crate::cvars` already owns the string table.
///
/// **Yards, and read as the CVar's FLOAT field.** The record carries both `+0x24` float and `+0x28`
/// int, written from the same string at registration; the range gate reads the float (`0x626810`)
/// and the periodic gates read the int. `0` is a real value and means *silence this class* —
/// `dist² < 0` is never true — which is why nothing here clamps to a floor.
#[derive(Resource, Clone, Copy)]
pub(crate) struct CombatLogRanges {
    /// By [`UnitClass`] index `0..=9`. Classes 0/1 (you and your pet) have no CVar and sit at the
    /// reference's `100000.0` sentinel; class 9 (unresolvable) sits at its `0.0`.
    class: [f32; 10],
    /// `CombatDeathLogRange` — the death line's own range, and the **only** formatter that has
    /// one. Not part of the `0x8629e0` table.
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

    /// The death line's live range — `CombatDeathLogRange`, for every class alike.
    ///
    /// **It really is every class.** The death formatter `0x62c160` looks the CVar up first
    /// (`0x62c19c`) and falls back to the per-class getter only when the *lookup* fails; the CVar
    /// is registered at startup, so the fallback is unreachable in a running client. So your own
    /// death passes on distance 0 rather than on class 0's sentinel, and an unresolvable unit's
    /// death is logged at 60 yd rather than refused by class 9's `0.0`.
    pub(crate) fn death(&self) -> f32 {
        self.death
    }

    /// Whether `name` is one of the eight — asked before [`Self::set`] so an observer holding
    /// the resource mutably does not flag a change it did not make.
    pub(crate) fn is_range_cvar(&self, name: &str) -> bool {
        name.eq_ignore_ascii_case(DEATH_LOG_RANGE_CVAR)
            || (0..self.class.len()).any(|i| {
                UnitClass::from_index(i)
                    .range_cvar()
                    .is_some_and(|c| c.eq_ignore_ascii_case(name))
            })
    }

    /// Apply one `SetCVar` to the table — `true` if the name was one of the eight.
    ///
    /// The class names are walked through [`UnitClass::range_cvar`] so this module keeps exactly
    /// one copy of them.
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

/// The combat log rows' change callback: the eight display ranges (yards, the
/// CVar's float field) and the periodic-effects switch.
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

/// **`CombatLogPeriodicSpells`** — "Log Periodic Effects", and its blast radius is wider than the
/// options row's wording.
///
/// Its first read site (`0x626dee`) is at the **top of the `SMSG_PERIODICAURALOG` handler**
/// `0x626dd0`, and a zero jumps to `0x6271b4`, a bare epilogue — so the whole packet body is
/// suppressed: every `CHAT_MSG_SPELL_PERIODIC_*` line **and** the floating DoT/HoT tick number
/// **and** the periodic miss-word. The other two sites are pure chat-line filters that leave the
/// floats alone: `0x62d9ae` (the `SMSG_SPELLNONMELEEDAMAGELOG` leg whose `periodicLog` byte is
/// set) and `0x62d25f` (`SMSG_SPELLORDAMAGE_IMMUNE`'s `IMMUNESPELL*` lines, only when the
/// packet's periodic byte is set).
///
/// Read as the CVar record's **int** `+0x28`, unlike [`CombatLogRanges`] which reads the float —
/// both fields are written from the same string at registration (`0x63e127` the int, `0x63e135`
/// the float).
///
/// A **missing record counts as OFF** in the reference, because the gate is
/// `cvar == NULL || cvar->int == 0`. That cannot happen here — the row is registered at startup —
/// and it is why the reference's own default `"1"` is load-bearing rather than incidental.
#[derive(Resource, Clone, Copy)]
pub(crate) struct LogPeriodicSpells(pub(crate) bool);

impl Default for LogPeriodicSpells {
    /// The reference's registered default, `"1"`.
    fn default() -> Self {
        Self(true)
    }
}

/// `CombatLogPeriodicSpells`' registered name.
pub(crate) const LOG_PERIODIC_CVAR: &str = "CombatLogPeriodicSpells";

/// `CombatDeathLogRange`'s registered name and default — `0x626d5f`, default string `"60"`
/// (`0x862e14`).
pub(crate) const DEATH_LOG_RANGE_CVAR: &str = "CombatDeathLogRange";
pub(crate) const DEATH_LOG_RANGE_DEFAULT: f32 = 60.0;

/// A unit's standing relative to the active player — the `0..9` index every combat-log selector in
/// the reference takes, in both parameter positions.
///
/// **The indices are VERIFIED by name, not inferred from behaviour.** `0x626810` (the combat-log
/// display-range getter) indexes a 10-entry table of `{cvarName, defaultValue}` pairs at
/// `DAT_008629e0`, and those cvar names *are* the class names — read out of `WoW.exe` at
/// `0x8629e0` (file offset `0x4629e0`):
///
/// | idx | cvar | default | meaning |
/// |---|---|---|---|
/// | 0 | *(NULL)* | `100000.0` (`[0x80dcc4]`) | the active player |
/// | 1 | *(NULL)* | `100000.0` | the active player's pet |
/// | 2 | `CombatLogRangeParty` | `50` | a party member |
/// | 3 | `CombatLogRangePartyPet` | `50` | a party member's pet |
/// | 4 | `CombatLogRangeFriendlyPlayers` | `50` | any other friendly player |
/// | 5 | `CombatLogRangeFriendlyPlayersPets` | `50` | that player's pet |
/// | 6 | `CombatLogRangeHostilePlayers` | `50` | a hostile player |
/// | 7 | `CombatLogRangeHostilePlayersPets` | `50` | that player's pet |
/// | 8 | `CombatLogRangeCreature` | `30` | a creature |
/// | 9 | `""` (the empty static `0x882748`) | `0.0` (`[0x7ffd74]`) | anything else — never logged |
///
/// Index 9's range of **zero** is why the msgType selectors can treat 8 and 9 alike: a class-9
/// endpoint never passes the range gate, so it never reaches them. Index 0/1's `100000.0` is the
/// "no gate" sentinel — your own lines are never dropped for distance.
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
    /// Unresolvable — not streamed, or a guid family with no unit behind it. Range `0.0`: the
    /// reference logs nothing for it, and neither do we.
    Unknown = 9,
}

impl UnitClass {
    /// The `0..=9` index back to its class — the inverse of the discriminant, for walking the
    /// reference's own table order.
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

    /// The CVar naming this class's display range, `None` for the two ungated classes (0/1).
    ///
    /// **This is the production lookup** — `CombatLogRanges::set` walks the classes through it to
    /// find the one a `SetCVar` names, so the table above is the only place these seven names are
    /// written down. It was `#[cfg(test)]` until the CVars were registered, with a comment saying
    /// it "becomes the production lookup the day the live CVar table is wired in".
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

    /// The **registered default** range in yards — the `defaultValue` half of the reference's own
    /// `{cvarName, defaultValue}` pairs at `0x8629e0`, and therefore what [`CombatLogRanges`] seeds
    /// itself with and what `cvars::REGISTERED` ships. The live value is the CVar's.
    pub(crate) fn default_range(self) -> f32 {
        match self {
            Self::Me | Self::MyPet => 100_000.0,
            Self::Creature => 30.0,
            Self::Unknown => 0.0,
            _ => 50.0,
        }
    }

    /// `true` for the local player — the `SELF` half of every `…SELFOTHER`/`…OTHERSELF` key.
    ///
    /// **Only class 0.** Your own pet is class 1 and is an `OTHER` for string-selection purposes:
    /// the reference says "Your pet hits X" through the same `…OTHEROTHER` template it uses for a
    /// stranger, and routes it to `COMBAT_PET_HITS` by the *msgType* selector instead. The two
    /// classifications are independent, which is the whole reason the selectors take the class
    /// index and the string picker takes a pair of booleans.
    fn is_me(self) -> bool {
        self == Self::Me
    }
}

/// Classify one guid against the active player — the reference's `0x5efea0(ecx = GUID*)`.
///
/// **The rule is settled at the bytes**, not inferred, and settling it corrected 1571's first cut
/// on three points, each of which changed behaviour:
///
/// - **The owner field is `UNIT_FIELD_CHARMEDBY`, then `UNIT_FIELD_CREATEDBY`.** We read
///   `SUMMONEDBY` first, copying the floating text's source classifier — a different field, and a
///   charmed unit was therefore judged on its own faction instead of its charmer's.
/// - **"Party" is the four party slots, NOT the raid roster.** A raid member outside your own
///   subgroup is class 4/5 (another friendly player), not class 2/3. We keyed off the whole
///   `GroupState` roster, so in a raid every one of 39 other players was "party".
/// - **Friend-or-foe is `CanAttack`, not a reaction rank.** A same-faction duel opponent reads
///   friendly by reaction and is unambiguously hostile here — which is the case the msgType
///   matrix's own duel arms exist for, so getting it from the reaction was self-defeating.
///
/// One half of the verdict is **not** implemented and is deliberately named rather than quietly
/// dropped: the reference runs `0x606980 CanAttack` **mutually**, in both directions, and
/// [`crate::target::ring::can_attack_from_player`] is specialised to the local player as the
/// attacker (1530). We run the direction we have. It differs only for a unit that can attack you
/// while you cannot attack it, which needs the general two-unit form to answer.
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
    // The owner: CHARMEDBY first, then CREATEDBY (`0x5f000c`/`0x5f0019`). A
    // pet/guardian/totem/charmed unit is classified by WHOSE it is, one rung above its own faction.
    let owner = store
        .0
        .unit_charmed_by()
        .or_else(|| store.0.unit_created_by());
    let owned_by = |g: u64| owner == Some(g);

    if owned_by(me) {
        return UnitClass::MyPet;
    }
    // **Party, not raid.** The reference's party slots hold your own subgroup; a raid member in
    // another subgroup is just another friendly player. `flags` bits 0-2 are the subgroup on both
    // sides of this comparison (`GroupMemberEntry::flags` / `GroupState::own_flags`), so an
    // ordinary 5-man party — where everyone is subgroup 0 — needs no special case.
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

    // Friend or foe — `CanAttack`, judged on **this unit**, never on its owner
    // (`0x5f00e3`/`0x5f00f2`). The owner hop was ours and it was wrong: the reference asks the pet
    // itself.
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
    // A creature with a PLAYER owner is that player's pet; a creature owned by nothing (or by
    // another creature) is just a creature.
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

/// `0x62a0d0` / `0x62a2e0` — the melee family's `(attacker, victim) → chat type`, `None` where the
/// reference returns `0x5e` (94, one past the end of the 94-entry type table = "no type", checked
/// at the emit `0x626850`).
///
/// Read off the decompiled selectors and cross-checked against the reference's 94-entry default
/// colour table (`0x804710`, whose rows `GetChatTypeIndex` numbers from 1 — so the `0x1b` the
/// selector returns is that table's row 28, `COMBAT_SELF_HITS`). `miss` picks the odd twin: every
/// HITS type is immediately followed by its MISSES type, which is why the reference's two selectors
/// differ only by `+1`.
///
/// **The two reclassifying arms are real and they are the duel/PvP case.** A *party* player (2) or
/// a *friendly* player (4) whose victim is you, your pet, or a party member is reported in the
/// HOSTILEPLAYER bucket — the client decides "attacking me makes you hostile" at the log, without
/// consulting faction. Their pets (3 and 5) get no such treatment.
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

/// `0x627820` and its damage sibling — the direct-spell family's `(attacker, victim) → chat type`.
///
/// Byte-identical in shape to [`combat_kind`] (same ten source arms, same two reclassifying arms,
/// same victim split for a creature source); only the base row differs, and `buff` picks the odd
/// twin exactly as `miss` does there. A *heal*, a *power gain* and an *aura* are all "BUFF"; damage
/// and every failed-to-land outcome are "DAMAGE".
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

/// `0x627d80` (damage) / `0x6274a0` (buffs) — the periodic family, and it is **a different shape**.
///
/// Ten rows, not sixteen: there is no PET bucket and no `CREATURE_VS_*` split, so a pet folds into
/// its owner's row and every creature source lands on one `SPELL_PERIODIC_CREATURE_*` row.
///
/// **Both selectors take ONE class, and WHICH one is the caller's to know**. This
/// doc used to say the endpoint was "the *source* alone", and it is not: `0x626630` fills classA
/// (the sentence's subject, the caster) and classB (the target), and the four periodic formatters
/// disagree about which one they hand this selector.
///
/// | formatter | msg-id argument | the byte |
/// |---|---|---|
/// | `PERIODICAURADAMAGE*` `0x628100` | **classB — the TARGET** | `0x628235 mov ecx,edi` where `edi = [ebp-0x10]` is `outClassB` |
/// | `PERIODICAURAHEAL*` `0x627240` | **classB — the TARGET** | `0x62732c mov ecx,[ebp-0x4]`, its own `outClassB` |
/// | `POWERGAIN*` `0x627520` | classA — the caster | `0x6275fb mov ecx,esi`, `esi = [ebp-0x18]` = `outClassA` |
/// | `SPELLPOWERLEECH*`/`…DRAIN*` `0x627930` | classA — the caster | `0x627a0f`/`0x627a3a mov ecx,esi`, `esi = [ebp-0x10]` = `outClassA` |
///
/// Passing the caster for the first two is not a cosmetic slip: a creature's DoT ticking **you**
/// types as `SPELL_PERIODIC_CREATURE_DAMAGE` instead of `SPELL_PERIODIC_SELF_DAMAGE`, and your own
/// DoT on a creature types as SELF instead of CREATURE — the two swap. Every addon that parses the
/// combat log by event name (MikScrollingBattleText matches a *different GlobalString pattern* per
/// event) then finds no pattern for either line and drops both.
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

/// `0x628980` — the death pair's selector, and it takes the **victim's** class alone.
///
/// The comparison is SIGNED (`jl` then `cmp ecx,5; jle`), which is why the negative arm is spelled
/// out: `0 <= c <= 5` is FRIENDLY_DEATH, everything else — a hostile player, their pet, any
/// creature, an unresolvable unit — is HOSTILE_DEATH. Our `UnitClass` cannot be negative, so the
/// sign only matters as a statement of what the arm is.
pub(crate) fn death_kind(victim: UnitClass) -> ChatEventKind {
    if (victim as u8) <= 5 {
        ChatEventKind::CombatFriendlyDeath
    } else {
        ChatEventKind::CombatHostileDeath
    }
}

/// `0x62b7d0` — where an aura's DEPARTURE is logged, off the bearer's class alone. Three rows, not
/// ten: `{0,1}` SELF · `{2,3}` PARTY · everything else OTHER.
///
/// The arrival is not this selector's — an aura landing rides the two PERIODIC selectors instead
/// (harmful → [`periodic_kind`]'s damage row, helpful → its buff row), which is the reference's own
/// asymmetry (`0x62b480`) and not a simplification of ours.
pub(crate) fn aura_gone_kind(bearer: UnitClass) -> ChatEventKind {
    use ChatEventKind as K;
    use UnitClass as C;
    match bearer {
        C::Me | C::MyPet => K::SpellAuraGoneSelf,
        C::Party | C::PartyPet => K::SpellAuraGoneParty,
        _ => K::SpellAuraGoneOther,
    }
}

/// `0x62c140` — the damage-shield two-way, off one class: `{0,1}` ON_SELF, else ON_OTHERS.
///
/// It has **two** users, which is the surprising half: the shield formatter itself, and *every*
/// `SMSG_SPELLLOGMISS` line (`0x5e7f31 push 1` routes `0x62bab0` here instead of the eight-row
/// spell matrix — a byte fact; whether it is deliberate is not derivable from the binary).
pub(crate) fn damage_shield_kind(subject: UnitClass) -> ChatEventKind {
    if matches!(subject, UnitClass::Me | UnitClass::MyPet) {
        ChatEventKind::SpellDamageShieldsOnSelf
    } else {
        ChatEventKind::SpellDamageShieldsOnOthers
    }
}

/// The HITS → MISSES step. Every combat pair is adjacent in the type table, so the reference's two
/// selectors are one table apart; this is that `+1`, spelled as the pairing it encodes.
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

/// The DAMAGE → BUFF step — [`miss_twin`]'s spell-family twin, same `+1` in the type table.
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

/// Which of a family's four templates applies — the reference's 45 `char*(self, other, *out)`
/// selectors (`0x629f90`, `0x62a290`, …), whose two arguments are booleans ("the subject is not
/// me", "the object is not me") and whose `*out` variant code `0..3` is what tells the caller how
/// many names to push.
///
/// `SelfSelf` is the code-`0` case for most families: the selector returns NULL and **no line is
/// produced**. Some families do define the key (`SPELLLOGSELFSELF`, `HEALEDSELFSELF`,
/// `PERIODICAURAHEALSELFSELF`); the ones that don't simply miss the lookup, which lands on the same
/// silence by the same route the reference's "string not found" arm takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Variant {
    /// I did it to someone else — `…SELFOTHER`, variant code 2.
    SelfOther,
    /// Someone else did it to me — `…OTHERSELF`, code 1.
    OtherSelf,
    /// Someone else, to someone else — `…OTHEROTHER`, code 3.
    OtherOther,
    /// Me, to me — `…SELFSELF`, code 0 for the families that have no such key.
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

    /// Whether the subject is spelled by name in this variant (rather than as "You"/"Your").
    fn names_subject(self) -> bool {
        matches!(self, Self::OtherSelf | Self::OtherOther)
    }

    /// Whether the object is spelled by name in this variant.
    fn names_object(self) -> bool {
        matches!(self, Self::SelfOther | Self::OtherOther)
    }

    /// Whether the SUBJECT is the local player — the single bit a [`Keying::Duo`] family keys on,
    /// since such a family has only one endpoint in its sentence.
    fn subject_is_me(self) -> bool {
        matches!(self, Self::SelfSelf | Self::SelfOther)
    }
}

/// How a family builds its GlobalString KEY out of the stem and the variant — the reference's three
/// shapes, and they are not interchangeable.
///
/// The 4-way `…SELFSELF`/`…SELFOTHER`/`…OTHERSELF`/`…OTHEROTHER` selector (`0x62a290` and its 44
/// siblings) is only the *commonest* one. A family whose sentence has a single participant —
/// a death, an aura landing or leaving, a tradeskill create — keys on that one endpoint with a
/// two-way `…SELF`/`…OTHER` (`0x62c160`, `0x62b480`, `0x629610`, and the other inline 2-ways), and
/// a handful of families have no variant at all (`DURABILITYDAMAGE_DEATH`, `PET_LOYALTY_GAIN`,
/// `SELFKILLOTHER`). Modelling all three here is what lets one composer serve the whole block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Keying {
    /// `…SELFSELF` / `…SELFOTHER` / `…OTHERSELF` / `…OTHEROTHER`, off both endpoints.
    Quad,
    /// Two keys off the SUBJECT alone. The words are the family's own — most spell `SELF`/`OTHER`,
    /// but `TRADESKILL_LOG`/`FEEDPET_LOG` say `_FIRSTPERSON`/`_THIRDPERSON`.
    Duo {
        me: &'static str,
        other: &'static str,
    },
    /// One key: the stem (plus any tail) IS the whole name.
    Single,
}

/// One value slot in a family's format string, in the order the shipped template consumes it.
///
/// `Attacker` and `Victim` are **conditional** — present only when [`Variant::names_subject`] /
/// [`Variant::names_object`] says the template spells that endpoint out. Everything else is
/// unconditional. That conditionality is exactly what makes a single ordered list describe all four
/// variants of a family: "Your %s hits %s for %d." and "%s's %s hits %s for %d." are the same
/// `[Attacker?, Spell, Victim?, Amount]`, once with the first slot dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Slot {
    /// `%s` — the subject's name, only when the variant spells it.
    Attacker,
    /// `%s` — the object's name, only when the variant spells it.
    Victim,
    /// `%s` — the spell's name.
    Spell,
    /// `%s` — the damage school's lowercase word (`SPELL_SCHOOL<n>_NAME`).
    School,
    /// `%s` — a power's word (`MANA_POINTS`/`RAGE_POINTS`/`FOCUS_POINTS`/`ENERGY_POINTS`).
    Power,
    /// `%d` — the primary amount.
    Amount,
    /// `%d` — a second amount (the leech family's "You gain %d").
    Amount2,
    /// `%s` — a second power word (the leech family's gained power).
    Power2,
    /// `%s` — a name the ARM already resolved and that the template always spells out: an item
    /// ("You create %s."), a gameobject ("You perform %s on %s."), a pet, a faction, or a cast
    /// failure's reason. Unconditional — unlike [`Self::Attacker`]/[`Self::Victim`] it is never
    /// dropped by the variant, because it names something that is never "you".
    Named,
}

/// A combat-log message family: the stem its four keys are built from, and the ordered slots its
/// templates consume.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Family {
    pub stem: &'static str,
    /// How the stem becomes a key (see [`Keying`]).
    pub keying: Keying,
    /// Appended AFTER the variant word — `AURAADDED` + `SELF` + `HARMFUL`, `SPELLEXTRAATTACKS` +
    /// `SELF` + `_SINGULAR`. Empty for every other family.
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

    /// Whether this family's template spells the SUBJECT out for `variant`.
    fn names_subject(&self, variant: Variant) -> bool {
        match self.keying {
            Keying::Quad => variant.names_subject(),
            // The two-way key IS the test: the `…OTHER` half is the one that carries a name.
            Keying::Duo { .. } => !variant.subject_is_me(),
            // A fixed key like `PARTYKILLOTHER` already says "other" in its name.
            Keying::Single => true,
        }
    }

    /// Whether this family's template spells the OBJECT out for `variant`.
    fn names_object(&self, variant: Variant) -> bool {
        match self.keying {
            Keying::Quad => variant.names_object(),
            // A Duo family's sentence has one participant; a `Victim` slot in one would be a
            // declaration bug, and `tests` is what catches it.
            Keying::Duo { .. } => false,
            Keying::Single => true,
        }
    }
}

/// The six suffixes `0x628410` can append to a finished sentence, in its fixed order.
///
/// **This is a separate pass over the already-formatted line, not more `%s` slots** — the reference
/// builds the sentence, appends what applies, and only then hands the whole thing to `0x626850` as
/// a single `%s`. Each trailer is skipped silently when its GlobalString resolves empty, so a
/// locale that ships one blank simply loses that clause.
///
/// Only four call sites can grow one, and what each can show differs because it passes zeroes for
/// the fields it has no wire source for: melee `COMBATHIT*` is the only family that can ever
/// show GLANCING/CRUSHING/BLOCK; `SPELLLOG*` can show RESIST/VULNERABLE/BLOCK/ABSORB;
/// `PERIODICAURADAMAGE*` and `VSENVIRONMENTALDAMAGE_*` RESIST/VULNERABLE/ABSORB.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Trailers {
    pub absorbed: u32,
    /// **Signed on purpose**: a negative resist is a *vulnerability* bonus and words itself as one.
    pub resisted: i32,
    pub blocked: u32,
    /// The swing's raw `HitInfo` — only `0x4000` GLANCING and `0x8000` CRUSHING are read, and only
    /// the melee call site passes a real one.
    pub hit_info: u32,
}

/// Append the trailers `0x628410` would, in its order, skipping each whose GlobalString is empty.
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

/// The value bound to each [`Slot`] for one line. Built by the caller from the packet; the family's
/// slot list decides which of these are read and in what order.
///
/// **The school and power slots hold INDICES, not words.** They are resolved to text inside
/// [`compose_line`], the same place the template itself is resolved and for the same reason: both
/// are localized strings off the install, and the packet arm that fills this struct has no VM in
/// hand. `None` in either where the family asks for it drops the line rather than printing a gap.
#[derive(Clone, Debug, Default)]
pub(crate) struct Fills {
    pub attacker: String,
    pub victim: String,
    pub spell: String,
    /// The damage school index (`SpellSchools`: 0 physical … 6 arcane) → `SPELL_SCHOOL<n>_NAME`.
    pub school: Option<u8>,
    /// The vmangos `Powers` index (0 mana … 3 energy) → `MANA_POINTS` and kin.
    pub power: Option<u32>,
    pub amount: i64,
    pub amount2: i64,
    /// The leech family's *gained* power index — usually the same as [`Self::power`].
    pub power2: Option<u32>,
    /// The [`Slot::Named`] text — an item, gameobject, pet, faction or failure reason, already
    /// localized by the arm that built this.
    pub named: String,
    /// The `0x628410` suffixes, for the four families that can grow one. `None` everywhere else.
    pub trailers: Option<Trailers>,
}

/// Resolve a family + variant to a finished line, or `None` when the key is absent from the
/// install's `GlobalStrings.lua` (the reference's "Warning: string %s not found" arm — no line).
///
/// The `%s`/`%d` walk is `vsnprintf` with a fixed argument list, which is what the reference does
/// at `0x626850`; 1.12's templates carry no positional (`%1$s`) specifiers, and could not — the
/// MSVC CRT the client links has never supported them.
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
            // A mismatch means our declared slot order disagrees with the shipped template — a
            // real defect, not a data condition, so it is loud. `tests` proves it cannot happen for
            // the enUS file; a locale that reshaped a string would land here rather than printing
            // a mangled sentence.
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

/// `vsnprintf` over the `%s`/`%d` subset the combat-log templates use. `None` on any mismatch —
/// too few arguments, or a `%d` where a string is queued (and vice versa).
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
            // Any other conversion is outside the vocabulary these templates use; treating it as
            // a mismatch is better than silently emitting a half-filled sentence.
            _ => return None,
        }
    }
    next.next().is_none().then_some(out)
}

/// A GlobalString off the VM's globals, `None` when absent **or empty** — the same two tests
/// `0x703bf0`'s callers make (`GetPVPRankInfo` at `0x51aa1c`/`0x51aa20` is the pattern
/// [`crate::ui_script`] already follows for the pvp strings).
pub(crate) fn global_string(script: &benilla_ui::script::UiScript, key: &str) -> Option<String> {
    script
        .lua()
        .globals()
        .get::<Option<String>>(key)
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
}

/// The school word a `…SCHOOL…` template's `%s` takes — **capitalized**.
///
/// The reference does not read a GlobalString here at all: `0x6264b0(schoolIndex)` indexes
/// `Resistances.dbc` (row array `[0xc0d9a4]`, 7 rows of 0x30 bytes, localized name at
/// `row + 0xc + locale*4`) — and those seven rows are
/// `Physical · Holy · Fire · Nature · Frost · Shadow · Arcane`. `SPELL_SCHOOL<n>_NAME` is a
/// *different* table in GlobalStrings.lua and holds the same seven words **lowercased**
/// (`GlobalStrings.lua:4102-4114`), which is what this used to return: "12 nature damage" where
/// the reference writes "12 Nature damage".
///
/// It reads as a nicety and is not one. MikScrollingBattleText classifies a parsed line's damage
/// school by an exact string compare against `SPELL_SCHOOL<n>_CAP`
/// (`MikCombatEventHelper.lua:3471-3492`) — an addon written against the live client's own output,
/// so it is independent evidence for the capital. A lowercase word falls through to
/// `DAMAGETYPE_UNKNOWN` and every school-coloured number in the addon loses its tint.
///
/// **Named residue:** the *source* is still the GlobalString, not the DBC. `SPELL_SCHOOL<n>_CAP`
/// carries the same seven strings as `Resistances.dbc` in enUS (both read, 2127), and this client
/// is enUS-only by construction (`combat_text::law::WORDS` and the rest of the string data are
/// hardcoded enUS). A locale where the two tables disagree would diverge; a `Resistances.dbc`
/// catalog is what closes it, and it is a data change rather than a string one — `compose_line`
/// is handed the Lua VM and nothing else.
pub(crate) fn school_word(script: &benilla_ui::script::UiScript, school: u8) -> Option<String> {
    global_string(script, &format!("SPELL_SCHOOL{school}_CAP"))
}

/// The GlobalString key `0x6278f0` resolves for a power tag, or `None` where it answers NULL.
///
/// The function is five lines and both of them matter: `cmp ecx,5; jae` — **unsigned**, so a tag
/// of 5 or more (and a negative one, which reads as huge) returns NULL — else
/// `[4*ecx + 0x85645c]` into `0x703bf0`. The table's five entries are the five `Powers` the wire
/// carries, **happiness included**: it is `HAPPINESS_POINTS = "Happiness"` (shipped enUS
/// `GlobalStrings.lua:2117`), not a hole. This file used to stop the table at energy and call
/// happiness "no GlobalString", which dropped every line the reference words with it — the
/// generic leech/drain fall-through out of `0x627de0`, and a happiness `POWERGAIN` tick.
///
/// What happiness really lacks is a **`COMBAT_TEXT_UPDATE`** tag: `0x627520`/`0x627930`'s four
/// `0x64a4c0` compares match only the other four nouns, so it produces the chat line and no
/// floating text. That is a different table, further down, after the emit.
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

/// Whether `0x6278f0` answers a noun at all — the `cmp ecx,5; jae` bound, without a VM in hand.
///
/// The formatters gate on the returned pointer (`627964 test edi,edi; 627966 je`) long before they
/// reach a template, so a caller that has no script yet still has to be able to ask.
pub(crate) fn power_has_word(power: u32) -> bool {
    power_key(power).is_some()
}

/// The power word a `POWERGAIN`/`SPELLPOWERLEECH`/`SPELLPOWERDRAIN` template takes, by the vmangos
/// `Powers` index the wire carries — `0x6278f0`'s table, resolved through the same GlobalString
/// mechanism the reference uses ([`power_key`] carries the law).
pub(crate) fn power_word(script: &benilla_ui::script::UiScript, power: u32) -> Option<String> {
    global_string(script, power_key(power)?)
}

/// Resolve one endpoint's display name — the reference's `GetObjectName` (`0x6264e0`), which is the
/// same ask-once name cache every other client-composed chat line waits on. `None` = not yet
/// answered; the caller re-tries next frame, exactly as the reference's deferred-name queue
/// (`DAT_00c4e208`, drained by the name-ready callback `0x6294b0`) replays its message.
///
/// `unit` is the endpoint's descriptor when it is streamed: `0x6264e0` looks the guid up in the
/// object manager first and, for a unit, calls `GetUnitName` (`0x609210`) — which keys a pet's or a
/// companion's name off its descriptor, not its guid ([`NameCache::resolve_unit`]).
pub(crate) fn object_name(
    guid: u64,
    unit: Option<&ObjectStore>,
    names: &NameCache,
    commands: &NetCommands,
) -> Option<String> {
    // Guid 0 = "the name is already in the fills" — no wire endpoint is ever guid 0, so the
    // sentinel costs nothing and is what lets `/chattest` drive the real drain with literal names.
    if guid == 0 {
        return None;
    }
    names.resolve_unit(guid, unit, commands).map(str::to_owned)
}

// ────────────────────────────────── the families ──────────────────────────────────────

/// Declare a family: its key stem and the ordered slots the shipped templates consume.
///
/// The order is read off the enUS `GlobalStrings.lua` and each declaration carries that line, so a
/// reader can check it without an install. `tests::every_family_matches_the_shipped_template` is
/// what checks it *with* one.
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

/// Declare a **two-way** family — one keyed on the subject alone (`Keying::Duo`). `$me`/`$other`
/// are the family's own words, which are `"SELF"`/`"OTHER"` for most and `"_FIRSTPERSON"`/
/// `"_THIRDPERSON"` for the two tradeskill ones. The optional `tail` is what `AURAADDED` and
/// `SPELLEXTRAATTACKS` append after it.
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

/// Declare a family with **no variant at all** — the key is the whole name.
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
/// Declare one `VSENVIRONMENTALDAMAGE_<TYPE>_{SELF,OTHER}` pair. The reference builds the key by
/// `snprintf("VSENVIRONMENTALDAMAGE_%s_%s", envTypeName, self ? "SELF" : "OTHER")` over the 6-entry
/// table `0x80dcac`; six declared families are that `snprintf` spelled out, which is what lets the
/// sweep check all twelve keys against the shipped file.
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

/// The environmental family for a `SMSG_ENVIRONMENTALDAMAGELOG` damage type, `None` for a byte
/// outside the six the table holds (`0x62abc5` indexes it unguarded; we decline instead).
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

/// Every family this module can emit — the sweep [`tests`] uses to check each one against the
/// shipped `GlobalStrings.lua`. Adding a family without adding it here is the only way to get an
/// unchecked slot order, so the list is the gate.
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

/// The `SpellMissInfo` byte `SMSG_SPELLLOGMISS` (and `SpellGo`'s own list) carries → the family
/// that words it. vmangos `SpellDefines.h:160-174`; the reference reaches the same ten families
/// through its own miss switch.
///
/// There is no `None` arm: the reference's jump table sends `0`, `1`, `10` and everything past the
/// enum to one default, `SPELLMISS*`. `0` is `SPELL_MISS_NONE` and should never appear in a miss
/// list, but if one arrives the reference words it as a miss rather than dropping it.
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
        // **10 ABSORB lands on SPELLMISS, not SPELLLOGABSORB**, and so do 0, 1 and anything out of
        // range: `0x62bb50`'s `lea eax,[edi-2]; cmp eax,9; ja default` puts all four on the
        // default arm `0x62c0e0`. 1571 had 10 on `SPELLLOGABSORB` — a family this switch never
        // reaches (it belongs to the direct-damage path) — and dropped 0/1 entirely.
        _ => SPELLMISS,
    }
}

/// The melee family a swing's `HitInfo` + `VictimState` select — the reference's display dispatcher
/// `0x629b60`, arm for arm, and its order is load-bearing because the tests overlap.
///
/// ```text
///   HitInfo & 0x10 (MISS)          → MISSED
///   VictimState == 5 (BLOCKS)      → VSBLOCK
///   damage == 0 && HitInfo & 0x20  → VSABSORB        (ABSORB)
///   damage == 0 && HitInfo & 0x40  → VSRESIST        (RESIST)
///   VictimState == 1 && damage > 0 → COMBATHIT[CRIT][SCHOOL]
///   otherwise                      → the VictimState word, or NO LINE
/// ```
///
/// **The last arm can decline.** `0x62a710` is gated twice before it words anything: the 10-entry
/// flag table `0x8628f8` = `[0,0,1,1,0,1,1,1,1,0]` indexed by VictimState (`0x62a720`), then
/// `add eax,-2; cmp eax,6; ja` into the jump table `0x62a8ec` — so VictimState **0, 1, 4 and 9
/// emit no line at all**. This used to answer `MISSED` for them and call that "the
/// reference's own fall-through", which it is not: the reference stays silent, and a `MISSED`
/// there is a sentence the real client never prints.
///
/// The bit values are vmangos's `HitInfo` under the `> 1.9.4` conditional that is compile-time true
/// for 5875 (`Objects/UnitDefines.h:250-268`) and its `VictimState` (`:237-248`) — the same pair
/// [`crate::sound::combat`] and [`crate::combat_text`] already read, here named once instead of a
/// fourth set of bare literals.
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
    // `0x62a710`'s own two gates, and they are the whole arm. The flag table's `1`s are exactly
    // these five; VictimState 0 (UNAFFECTED), 1 with no damage, 4 (INTERRUPT) and 9 are `0`s and
    // the formatter returns having emitted nothing.
    //
    // The table's index 5 is a `1`, and it is unreachable: a block is taken above, which is why
    // the two readings have to be kept apart rather than collapsed into one list of five.
    // A VictimState of 10 or more is not bounds-checked before the table read — the load runs on
    // a wire `u32` and index 10 lands in the adjacent pointer table `0x862920`, whose entries are
    // all non-zero, so gate 1 *passes* and the second gate's `cmp eax,6; ja` is what rejects it.
    // No line, nothing fired — which is the answer `None` already gives.
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

/// A combat-log line with every decision already made and only its **names** outstanding.
///
/// This split is the reference's, not a convenience. `0x629b60` classifies and picks the family at
/// the packet; when a name the sentence needs is not in the cache yet, the message is pushed onto
/// the deferred queue at `DAT_00c4e208` and replayed by the name-ready callback `0x6294b0`. Doing
/// the same means the classification is made while both units are certainly streamed — a creature
/// that dies to the killing blow is gone from the object manager long before its name query
/// answers, and a classification deferred to that moment would read `Unknown` and drop the line.
#[derive(Clone, Debug)]
pub(crate) struct PendingCombat {
    pub kind: ChatEventKind,
    pub family: Family,
    pub variant: Variant,
    /// The guid whose name fills the `Attacker` slots — the sentence's SUBJECT, which for
    /// [`DAMAGESHIELD`] is the shield's bearer rather than the packet's `attacker` field.
    pub subject: u64,
    /// The guid whose name fills the `Victim` slots.
    pub object: u64,
    /// Everything the template needs that is not a name; the two name fields are filled at drain.
    pub fills: Fills,
    /// Where [`Slot::Named`]'s text comes from — see [`Named`].
    pub named: Named,
    pub tries: u16,
}

/// What still has to be looked up before a line's [`Slot::Named`] can be filled.
///
/// The reference has both hops and keeps them apart for the same reason: an item name comes from
/// the **item cache** (`0x55ba30`), whose miss parks the whole formatter on the deferred queue and
/// re-runs it when the server answers (`0x6294b0`); a unit name comes from `GetObjectName`
/// (`0x6264e0`), the same resolve the two endpoint slots already use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Named {
    /// Nothing to do — the arm already put the text in [`Fills::named`], or the family has no
    /// `Named` slot at all.
    Ready,
    /// An item entry, through the ask-once item cache.
    Item(u32),
    /// A unit or object guid, through the name cache.
    Unit(u64),
}

/// Build a queued line from an already-classified pair, or `None` when the reference would emit
/// nothing: an unroutable type (`0x5e`), or an endpoint whose class is out of the range gate.
///
/// `kind` is the caller's — the three msgType matrices differ per family, so the arm that knows
/// which packet it is decides, and this only assembles.
pub(crate) fn queue(
    kind: ChatEventKind,
    family: Family,
    subject: (u64, UnitClass),
    object: (u64, UnitClass),
    fills: Fills,
    named: Named,
) -> Option<PendingCombat> {
    // **A class-9 endpoint alone does NOT drop the line**, and 1571 had this wrong. Its range of
    // `0.0` means it can never satisfy *its own half* of the gate — but the gate is an OR over the
    // two endpoints (`0x626630`), so a resolvable one at the other end still carries the line. What
    // drops here is only the case the reference's own resolve step fails on: neither endpoint
    // resolvable at all.
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
