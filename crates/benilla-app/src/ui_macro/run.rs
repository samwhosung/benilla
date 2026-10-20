//! A macro body's runnable lines, and the spell it binds for the action bar.
//!
//! A macro slot shows the macro's icon but the cooldown, usability, range and checked state of its
//! bound spell, the record's `[rec+0x564]`, which the slot resolver `0x4e5a50` returns for a
//! macro. `0x4efe00` derives it from the first line that is a `/cast` alias or holds
//! `CastSpellByName(`, through the resolver the `CastSpellByName` binding (`0x4b4ab0`) uses
//! (`0x4b3950` → `0x4b3a10`), so a bare name binds the highest known rank.
//!
//! `[rec+0x568]` is the book the spell resolved from, 0 the player's and 1 the pet's: the name is
//! looked up in the player's book, then the pet's (`0x4b3a10`), and a pet match binds the pet's
//! spell, which the action bar then reads through the pet's legs (`ui_action::state`).

use benilla_ui::script::{resolve_spell_in_books, PetBookState, SpellBookState};

use crate::ui_chat::commands::{Command, SlashCommands, SlashIndex};

/// Arm B's literal (`0x84cab0`), searched case-sensitively anywhere in the line (`0x4efed5`).
const CAST_BY_NAME_CALL: &str = "CastSpellByName(";

/// Which of `0x4efe00`'s arms matched: an unresolved name stores -1 from arm A (`0x4eff48`) and
/// 0 from arm B (`0x4eff81`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CastArm {
    /// A `/cast`-alias line (`SLASH_CAST%d`, read as Lua globals at `0x703bf0`).
    Slash,
    /// A `CastSpellByName("…")` call anywhere in the line.
    ByName,
}

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

impl Bound {
    /// Whether `0x4f0ce0` derives this record again when the pet's spell list arrives
    /// (`0x4f0cfb`-`0x4f0d11`): a `/cast` that did not resolve (`[rec+0x564] < 0`), or any record
    /// whose book selector is the pet's (`[rec+0x568] != 0`), whatever it is bound to now. Any
    /// other record keeps its cast.
    pub(crate) fn follows_the_pet_book(self) -> bool {
        self.spell == BoundSpell::Unresolved || self.pet_book
    }
}

/// A macro body's runnable lines: the reference's tokenizer (`0x64ae50`) splits on either `\r` or
/// `\n` and skips empty tokens. Deviation: each line is trimmed, because the reference sends an
/// indented `" /say hi"` as plain chat, never as the command meant. A whitespace-only line is fired
/// there and dropped by `ChatEdit_SendText`, the same observable.
pub(crate) fn macro_lines(body: &str) -> impl Iterator<Item = &str> {
    body.split(['\r', '\n'])
        .map(str::trim)
        .filter(|l| !l.is_empty())
}

/// The spell name a macro body casts (`0x4efe00`): the first `/cast`-alias line, or line holding
/// `CastSpellByName(` and then a quote, whose text is the name even when it is empty
/// ([`quoted_call_argument`]). Deviation: the `/cast` argument is trimmed, because the reference
/// binds `/cast  Fireball` (two spaces) to a name that never resolves.
pub(crate) fn cast_name(table: &SlashCommands, body: &str) -> Option<(CastArm, String)> {
    for line in macro_lines(body) {
        // Arm A: the alias folds ASCII case (`_strnicmp`, `0x414310`, as `SlashCommands::lookup`
        // does), and the next byte must be a space (`0x4efe96`), so a tab or a bare `/cast` misses.
        if let Some(rest) = line.strip_prefix('/') {
            let (cmd, args) = rest.split_once(' ').unwrap_or((rest, ""));
            if table.lookup(cmd) == Some(Command::Slash(SlashIndex::Cast)) {
                let args = args.trim();
                if !args.is_empty() {
                    return Some((CastArm::Slash, args.to_string()));
                }
                // The reference tries arm B on this line, then the next line.
                continue;
            }
        }
        // Arm B, second, on the same line (`0x4efed5`): its quote decides the binding, so the walk
        // ends here whatever the name is or resolves to.
        if let Some(name) = quoted_call_argument(line) {
            return Some((CastArm::ByName, name));
        }
    }
    None
}

/// The text of the first quote after `CastSpellByName(` on the line: from the first `"` at or
/// after the match (`0x4efef0`-`0x4efefb`, the literal holds none) to the next `"` or the line's
/// end (`0x4eff58`-`0x4eff6d`). Whatever stands between the parenthesis and the quote is skipped,
/// so `CastSpellByName(IsShiftKeyDown() and "Frostbolt(Rank 1)" or "Frostbolt")` gives
/// `Frostbolt(Rank 1)`. A line with no such quote gives nothing and the walk goes on (`0x4efefd`);
/// an empty name is a name, which `0x4b3bc0` answers 0.
fn quoted_call_argument(line: &str) -> Option<String> {
    let after_call = &line[line.find(CAST_BY_NAME_CALL)?..];
    let open = after_call.find('"')?;
    let rest = &after_call[open + 1..];
    Some(rest.split('"').next().unwrap_or_default().to_string())
}

/// The macro's cast, derived again (`0x4efe00`) from `was`: [`cast_name`] resolved as
/// `CastSpellByName` resolves a name ([`resolve_spell_in_books`]), so the cooldown swirl and the
/// press agree on the rank. A match writes the book it came from; a miss leaves the selector.
pub(crate) fn bound_spell(
    table: &SlashCommands,
    body: &str,
    player: &SpellBookState,
    pet: &PetBookState,
    was: Bound,
) -> Bound {
    let Some((arm, name)) = cast_name(table, body) else {
        return Bound {
            spell: BoundSpell::None,
            ..was
        };
    };
    match (resolve_spell_in_books(player, pet, &name), arm) {
        (Some(s), _) => Bound {
            spell: BoundSpell::Spell(s.spell_id),
            pet_book: s.pet,
        },
        (None, CastArm::Slash) => Bound {
            spell: BoundSpell::Unresolved,
            ..was
        },
        (None, CastArm::ByName) => Bound {
            spell: BoundSpell::None,
            ..was
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_chat::commands::SlashCommands;
    use benilla_ui::script::SpellSlotView;

    /// The aliases these tests need, built through the global-string reader as boot builds it.
    fn table() -> SlashCommands {
        SlashCommands::build(
            |name| match name {
                "SLASH_CAST1" => Some("/cast".into()),
                "SLASH_CAST2" => Some("/spell".into()),
                "SLASH_SCRIPT1" => Some("/script".into()),
                "SLASH_TARGET1" => Some("/target".into()),
                _ => None,
            },
            |_| None,
        )
    }

    #[test]
    fn macro_lines_trims_blanks_and_windows_line_endings() {
        let body = "/cast Fireball\r\n\r\n  /say pew  \n";
        let lines: Vec<&str> = macro_lines(body).collect();
        assert_eq!(lines, ["/cast Fireball", "/say pew"]);
    }

    /// `/spell` is `SLASH_CAST2` in the shipped strings.
    #[test]
    fn cast_name_reads_the_first_cast_line_through_the_alias_table() {
        let t = table();
        assert_eq!(
            cast_name(&t, "/target Bob\n/cast Fireball\n/say pew"),
            Some((CastArm::Slash, "Fireball".into()))
        );
        assert_eq!(
            cast_name(&t, "/spell Frostbolt"),
            Some((CastArm::Slash, "Frostbolt".into()))
        );
        // The whole argument, rank included; `resolve_spell_by_name` parses it.
        assert_eq!(
            cast_name(&t, "/cast Fireball(Rank 1)"),
            Some((CastArm::Slash, "Fireball(Rank 1)".into()))
        );
        // First match wins.
        assert_eq!(
            cast_name(&t, "/cast Fireball\n/cast Frostbolt"),
            Some((CastArm::Slash, "Fireball".into()))
        );
        // A bare `/cast` binds nothing and does not stop the walk.
        assert_eq!(
            cast_name(&t, "/cast\n/cast Frostbolt"),
            Some((CastArm::Slash, "Frostbolt".into()))
        );
        assert_eq!(cast_name(&t, "/say hello\n/target Bob"), None);
    }

    #[test]
    fn the_alias_separator_is_a_literal_space_and_the_alias_folds_case() {
        let t = table();
        let fireball = Some((CastArm::Slash, "Fireball".into()));
        assert_eq!(cast_name(&t, "/CAST Fireball"), fireball);
        assert_eq!(cast_name(&t, "/Cast Fireball"), fireball);
        // A tab does not separate, so the walk goes on.
        assert_eq!(
            cast_name(&t, "/cast\tFireball\n/cast Frostbolt"),
            Some((CastArm::Slash, "Frostbolt".into()))
        );
        // Arm B is case-sensitive.
        assert_eq!(
            cast_name(&t, r#"/script castspellbyname("Fireball")"#),
            None
        );
    }

    #[test]
    fn cast_name_reads_a_quoted_cast_spell_by_name_call() {
        let t = table();
        assert_eq!(
            cast_name(&t, r#"/script CastSpellByName("Shadow Bolt")"#),
            Some((CastArm::ByName, "Shadow Bolt".into()))
        );
        // Spacing and a second argument don't matter; the first quoted argument is the name.
        assert_eq!(
            cast_name(&t, r#"/script CastSpellByName( "Healing Touch", 1 )"#),
            Some((CastArm::ByName, "Healing Touch".into()))
        );
        // A computed argument with no quote has no name to bind.
        assert_eq!(cast_name(&t, "/script CastSpellByName(spell)"), None);
    }

    /// `0x4efef0`-`0x4efefb` scan for the first quote anywhere after the match, whatever the
    /// parenthesis holds, and `0x4eff58`-`0x4eff6d` read to the next quote or the line's end.
    #[test]
    fn the_first_quote_after_the_call_is_the_name_wherever_it_stands() {
        let t = table();
        assert_eq!(
            cast_name(
                &t,
                r#"/script CastSpellByName(IsShiftKeyDown() and "Frostbolt(Rank 1)" or "Frostbolt")"#
            ),
            Some((CastArm::ByName, "Frostbolt(Rank 1)".into()))
        );
        assert_eq!(
            cast_name(&t, r#"/script CastSpellByName(GetSpell("Fire"))"#),
            Some((CastArm::ByName, "Fire".into())),
            "up to the next quote, the call's own closing parenthesis not a stop"
        );
        assert_eq!(
            cast_name(&t, r#"/script CastSpellByName("Fire"#),
            Some((CastArm::ByName, "Fire".into())),
            "an unterminated name runs to the line's end"
        );
        // The scan starts at the match: a quote ahead of it is not the name, and a line with none
        // after it goes on to the next line (`0x4efefd`).
        assert_eq!(
            cast_name(
                &t,
                "/script local a = \"x\"; CastSpellByName(spell)\n/cast Frostbolt"
            ),
            Some((CastArm::Slash, "Frostbolt".into()))
        );
    }

    /// The bound spell of `body` for a macro that was bound to nothing, against the player's book.
    fn bound(t: &SlashCommands, body: &str, book: &SpellBookState) -> BoundSpell {
        bound_spell(t, body, book, &PetBookState::default(), Bound::default()).spell
    }

    fn fireball_book() -> SpellBookState {
        SpellBookState {
            tabs: Vec::new(),
            slots: vec![SpellSlotView {
                spell_id: 133,
                name: "Fireball".into(),
                ..Default::default()
            }],
        }
    }

    /// An empty name is a name: `0x4eff7b` resolves it to 0 and `0x4eff81` stores that, ending the
    /// walk on its line, where a later `/cast` is never read.
    #[test]
    fn an_empty_name_binds_nothing_and_ends_the_walk() {
        let t = table();
        let body = "/script CastSpellByName(\"\")\n/cast Fireball";
        assert_eq!(cast_name(&t, body), Some((CastArm::ByName, String::new())));
        assert_eq!(bound(&t, body, &fireball_book()), BoundSpell::None);
        // A computed first argument binds the quoted fallback after it, not the later line.
        assert_eq!(
            bound(
                &t,
                "/script CastSpellByName(x and \"Fireball\" or \"Frostbolt\")\n/cast Frostbolt",
                &fireball_book()
            ),
            BoundSpell::Spell(133)
        );
    }

    #[test]
    fn bound_spell_resolves_through_the_book() {
        let book = SpellBookState {
            tabs: Vec::new(),
            slots: vec![
                SpellSlotView {
                    spell_id: 133,
                    name: "Fireball".into(),
                    rank: Some("Rank 1".into()),
                    ..Default::default()
                },
                SpellSlotView {
                    spell_id: 145,
                    name: "Fireball".into(),
                    rank: Some("Rank 2".into()),
                    ..Default::default()
                },
            ],
        };
        let t = table();
        // No subtext -> the highest known rank.
        assert_eq!(bound(&t, "/cast Fireball", &book), BoundSpell::Spell(145));
        // A pinned subtext -> that rank. The spaced form keeps its space in the name
        // (`0x4b3950` trims nothing), so it names no spell and the bar greys the slot.
        assert_eq!(
            bound(&t, "/cast Fireball(Rank 1)", &book),
            BoundSpell::Spell(133)
        );
        assert_eq!(
            bound(&t, "/cast Fireball (Rank 1)", &book),
            BoundSpell::Unresolved
        );
    }

    /// An unknown `/cast` is -1 (`0x4eff48`), which the bar greys; no cast line, or an unknown
    /// `CastSpellByName(`, is 0 (`0x4eff81`).
    #[test]
    fn an_unresolved_cast_is_arm_dependent() {
        let book = SpellBookState::default();
        let t = table();
        assert_eq!(bound(&t, "/cast Pyroblast", &book), BoundSpell::Unresolved);
        assert_eq!(
            bound(&t, "/spell Pyroblast\n/say pew", &book),
            BoundSpell::Unresolved
        );
        assert_eq!(bound(&t, "/say hi", &book), BoundSpell::None);
        assert_eq!(bound(&t, ".spawn 16032", &book), BoundSpell::None);
        assert_eq!(
            bound(&t, r#"/script CastSpellByName("Pyroblast")"#, &book),
            BoundSpell::None
        );
    }

    fn named(spell_id: u32, name: &str) -> SpellSlotView {
        SpellSlotView {
            spell_id,
            name: name.into(),
            ..Default::default()
        }
    }

    /// `0x4b3a10` searches the player's book, then the pet's, and `0x4b3b51` writes which one
    /// matched; `0x4efe00` hands that book to the record (`0x4eff2b`, `0x4eff72`).
    #[test]
    fn a_name_only_the_pets_book_holds_binds_its_spell_with_the_pets_book() {
        let t = table();
        let player = SpellBookState {
            tabs: Vec::new(),
            slots: vec![named(133, "Fireball"), named(1, "Seduction")],
        };
        let pet = PetBookState {
            token: None,
            slots: vec![named(6358, "Seduction"), named(19647, "Spell Lock")],
        };
        let derive = |body: &str| bound_spell(&t, body, &player, &pet, Bound::default());
        let from_pet = |id| Bound {
            spell: BoundSpell::Spell(id),
            pet_book: true,
        };
        assert_eq!(derive("/cast Spell Lock"), from_pet(19647));
        assert_eq!(
            derive(r#"/script CastSpellByName("Spell Lock")"#),
            from_pet(19647),
            "the quoted form binds it too"
        );
        // The player's book is searched first.
        assert_eq!(derive("/cast Seduction"), Bound::from(BoundSpell::Spell(1)));
        assert_eq!(
            derive("/cast Fireball"),
            Bound::from(BoundSpell::Spell(133))
        );
        // A name in neither book is what it was: -1 for a `/cast`, 0 for the quoted form.
        assert_eq!(derive("/cast Claw"), Bound::from(BoundSpell::Unresolved));
        assert_eq!(
            derive(r#"/script CastSpellByName("Claw")"#),
            Bound::from(BoundSpell::None)
        );
        // With the pet gone, a pet spell is a name in neither.
        let gone = |was| {
            bound_spell(
                &t,
                "/cast Spell Lock",
                &player,
                &PetBookState::default(),
                was,
            )
        };
        assert_eq!(gone(Bound::default()).spell, BoundSpell::Unresolved);
        // A miss leaves the book selector (`0x4b3a10` writes it on a match alone), so a record
        // that was the pet's stays the pet's through it, and a cast line that is gone keeps it too.
        assert!(gone(from_pet(19647)).pet_book);
        assert!(!gone(Bound::default()).pet_book);
        let gone_line = bound_spell(&t, "/say hi", &player, &pet, from_pet(19647));
        assert_eq!(
            gone_line,
            Bound {
                spell: BoundSpell::None,
                pet_book: true
            }
        );
    }

    /// `0x4f0ce0` derives a record again at a pet spell message only when `[rec+0x564] < 0` or
    /// `[rec+0x568] != 0` (`0x4f0cfb`-`0x4f0d11`), whatever it is bound to now.
    #[test]
    fn only_an_unresolved_or_pet_selected_record_follows_the_pets_list() {
        let record = |spell, pet_book| Bound { spell, pet_book };
        assert!(record(BoundSpell::Unresolved, false).follows_the_pet_book());
        assert!(record(BoundSpell::Spell(6358), true).follows_the_pet_book());
        assert!(
            record(BoundSpell::None, true).follows_the_pet_book(),
            "a quoted call that lost the pet's spell keeps the pet's book"
        );
        assert!(!record(BoundSpell::None, false).follows_the_pet_book());
        assert!(!record(BoundSpell::Spell(133), false).follows_the_pet_book());
    }
}
