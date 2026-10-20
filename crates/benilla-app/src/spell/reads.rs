//! What a spell computation read of the caster's live state, reported where it reads it, for a
//! caller that keeps the result and builds it again when one of those inputs moves: the spell
//! tooltip feed (`crate::ui_tooltip`). The reference computes at every call and keeps nothing.

/// A caster unit field a cost, cast-time or cooldown computation reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnitField {
    Level,
    BaseMana,
    BaseHealth,
    RangedTime,
    /// `UNIT_MOD_CAST_SPEED`, the cast-time factor (`0x6e342c`).
    CastSpeed,
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
}

/// The reads of a caller that keeps nothing.
pub(crate) struct Unrecorded;

impl SpellReads for Unrecorded {
    fn unit(&mut self, _: UnitField) {}
    fn item_search(&mut self, _: u32) {}
}
