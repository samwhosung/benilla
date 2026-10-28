//! The level a spell's per-level terms read.

/// The level a spell's per-level terms read, `0x6e3130` and `GetPowerCost`'s own copy of it
/// (`0x6e3242`-`0x6e3254`): the caster's `[vtbl+0xa8]` for the spell, over 5. Found only when a
/// term reads it.
#[derive(Clone, Copy)]
pub(crate) enum SpellLevel<'a> {
    /// The active player's ([`super::player_spell_level`]): its skill in the line of the spell
    /// `GetSkillSpell 0x5ea5d0` names, capped by that spell's `maxLevel`.
    Player {
        me: Option<&'a crate::net::ObjectStore>,
        skills: super::PlayerSkills<'a>,
    },
    /// A level already found: a pet's (`0x60cd80`).
    Known(u32),
}

impl SpellLevel<'_> {
    /// The level for `d`.
    pub(crate) fn read(self, d: &benilla_formats::SpellDisplay) -> u32 {
        match self {
            Self::Known(level) => level,
            Self::Player { me, skills } => super::player_spell_level(me, skills, d),
        }
    }
}
