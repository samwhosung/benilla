//! What a spell computation read of the caster's live state, reported where it reads it, for a
//! caller that keeps the result and builds it again when one of those inputs moves: the spell
//! tooltip feed (`crate::ui_tooltip`). The reference computes at every call and keeps nothing.

/// A caster unit field a cost, cast-time or cooldown computation reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnitField {
    BaseMana,
    BaseHealth,
    RangedTime,
    /// `UNIT_MOD_CAST_SPEED`, the cast-time factor (`0x6e342c`).
    CastSpeed,
    /// `UNIT_FIELD_POWER_COST_MODIFIER`, the seven schools' flat cost terms (`0x6e32ad`).
    PowerCostModifier,
    /// `UNIT_FIELD_POWER_COST_MULTIPLIER`, the seven schools' cost factors less 1 (`0x6e32c0`).
    PowerCostMultiplier,
}

impl UnitField {
    /// The field's bit in a mask of them.
    pub(crate) fn bit(self) -> u16 {
        1 << self as u16
    }
}

/// The reads a computation reports, each where it makes it.
pub(crate) trait SpellReads {
    /// It read the caster's `field`.
    fn unit(&mut self, field: UnitField);
    /// The equipped-item search (`0x5f0c50`) ran over the equipment slots in `slots`, asking the
    /// disarm flag of the hands among them.
    fn item_search(&mut self, slots: u32);
    /// It read the player's skill in `line`, the level of a per-level term ([`SpellLevel`]).
    fn skill_line(&mut self, line: u32);
}

/// The level a spell's per-level terms read, `0x6e3130` and `GetPowerCost`'s own copy of it
/// (`0x6e3242`-`0x6e3254`): the caster's `[vtbl+0xa8]` for the spell, over 5. Found only when a
/// term reads it.
#[derive(Clone, Copy)]
pub(crate) enum SpellLevel<'a> {
    /// The active player's ([`super::player_spell_level`]): its skill in the line of the spell
    /// `GetSkillSpell 0x5ea5d0` names, capped by that spell's `maxLevel`. A read of that line and
    /// of the hands it asked.
    Player {
        me: Option<&'a crate::net::ObjectStore>,
        skills: super::PlayerSkills<'a>,
    },
    /// A level already found: a pet's (`0x60cd80`), which is a pet input and recorded nowhere.
    Known(u32),
}

impl SpellLevel<'_> {
    /// The level for `d`, its read reported.
    pub(crate) fn read(self, d: &benilla_formats::SpellDisplay, reads: &mut dyn SpellReads) -> u32 {
        match self {
            Self::Known(level) => level,
            Self::Player { me, skills } => super::player_spell_level(me, skills, d, reads),
        }
    }
}

/// The reads of a caller that keeps nothing.
pub(crate) struct Unrecorded;

impl SpellReads for Unrecorded {
    fn unit(&mut self, _: UnitField) {}
    fn item_search(&mut self, _: u32) {}
    fn skill_line(&mut self, _: u32) {}
}
