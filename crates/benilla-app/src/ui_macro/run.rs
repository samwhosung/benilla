//! A macro body's runnable lines, and the app's view of the spell it binds for the action bar.
//!
//! A macro slot shows the macro's icon but the cooldown, usability, range and checked state of its
//! bound spell, the record's `[rec+0x564]`, which the slot resolver `0x4e5a50` returns for a
//! macro, and `[rec+0x568]`, the book it resolved from. The engine derives both
//! ([`benilla_ui::script::MacroBinding`]); the action bar reads them through [`Bound`]
//! (`ui_action::state`).

use benilla_ui::script::MacroBinding;

/// A macro record's cached bound spell, the reference's `[rec+0x564]` (written by `0x4efe00`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum BoundSpell {
    /// `0`: no cast line, or an unresolved `CastSpellByName(`: a usable macro with no spell state.
    #[default]
    None,
    /// `-1`: an unresolved `/cast`; the bar greys the slot (`0x4e5050` refuses it at `0x4e518b`).
    Unresolved,
    /// A spell id: the slot shows that spell's state.
    Spell(u32),
}

/// A macro record's cached cast: [`BoundSpell`] and the book selector `[rec+0x568]` beside it, 0
/// the player's book and 1 the pet's (`0x4b3b3d`, `0x4b3b51`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Bound {
    pub(crate) spell: BoundSpell,
    /// Written on a match alone, so a derivation that finds nothing leaves it: a macro that was
    /// bound to the pet's spell and lost it keeps the pet's book (`0x4eff14`, `0x4eff48`,
    /// `0x4eff81` store `+0x564` only).
    pub(crate) pet_book: bool,
}

impl From<BoundSpell> for Bound {
    /// A binding from the player's book.
    fn from(spell: BoundSpell) -> Self {
        Self {
            spell,
            pet_book: false,
        }
    }
}

impl From<MacroBinding> for Bound {
    /// The engine's record: 0 none, -1 unresolved, else the spell.
    fn from(b: MacroBinding) -> Self {
        Self {
            spell: match b.spell {
                0 => BoundSpell::None,
                s if s < 0 => BoundSpell::Unresolved,
                s => BoundSpell::Spell(s as u32),
            },
            pet_book: b.pet_book,
        }
    }
}

pub(crate) use benilla_ui::script::macro_lines;
