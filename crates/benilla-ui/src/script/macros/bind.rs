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
//! spell, which the action bar then reads through the pet's legs.

use super::MacroBinding;
use crate::script::{resolve_spell_in_books, PetBookState, SpellBookState};

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

/// A macro body's runnable lines: the reference's tokenizer (`0x64ae50`) splits on either `\r` or
/// `\n` and skips empty tokens. Deviation: each line is trimmed, because the reference sends an
/// indented `" /say hi"` as plain chat, never as the command meant. A whitespace-only line is fired
/// there and dropped by `ChatEdit_SendText`, the same observable.
pub fn macro_lines(body: &str) -> impl Iterator<Item = &str> {
    body.split(['\r', '\n'])
        .map(str::trim)
        .filter(|l| !l.is_empty())
}

/// The `/cast` aliases as `0x4efe00` reads them, each time it derives: the globals `SLASH_CAST1`,
/// `SLASH_CAST2`, … through `FrameScript_GetText` (`0x703bf0`), to the first that is absent or
/// empty.
pub(crate) fn cast_aliases(lua: &mlua::Lua) -> Vec<String> {
    (1..)
        .map_while(|n| crate::strings::global(lua, &format!("SLASH_CAST{n}")))
        .collect()
}

/// The spell name a macro body casts (`0x4efe00`): the first `/cast`-alias line, or line holding
/// `CastSpellByName(` and then a quote, whose text is the name even when it is empty
/// ([`quoted_call_argument`]). Deviation: the `/cast` argument is trimmed, because the reference
/// binds `/cast  Fireball` (two spaces) to a name that never resolves.
pub(crate) fn cast_name(aliases: &[String], body: &str) -> Option<(CastArm, String)> {
    for line in macro_lines(body) {
        // Arm A: an alias compared without case over its length (`SStrCmpI`, `0x64a4c0`), and the
        // next byte must be a space (`0x4efe96`), so a tab or a bare `/cast` misses.
        let after_alias = aliases.iter().find_map(|alias| {
            let head = line.get(..alias.len())?;
            let rest = line.get(alias.len()..)?;
            (head.eq_ignore_ascii_case(alias))
                .then_some(rest)
                .and_then(|rest| rest.strip_prefix(' '))
        });
        if let Some(args) = after_alias {
            let args = args.trim();
            if !args.is_empty() {
                return Some((CastArm::Slash, args.to_string()));
            }
            // The reference stores any alias hit's result and returns, -1 for a name that resolves
            // nothing (`0x4eff28`-`0x4eff48`); under the trim above, an argument of spaces alone
            // goes on to the next line instead.
            continue;
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
pub(crate) fn derive(
    aliases: &[String],
    body: &str,
    player: &SpellBookState,
    pet: &PetBookState,
    was: MacroBinding,
) -> MacroBinding {
    let Some((arm, name)) = cast_name(aliases, body) else {
        return MacroBinding { spell: 0, ..was };
    };
    match (resolve_spell_in_books(player, pet, &name), arm) {
        (Some(s), _) => MacroBinding {
            spell: s.spell_id as i32,
            pet_book: s.pet,
        },
        (None, CastArm::Slash) => MacroBinding { spell: -1, ..was },
        (None, CastArm::ByName) => MacroBinding { spell: 0, ..was },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::SpellSlotView;

    /// The aliases these tests need: the shipped `SLASH_CAST1` and `SLASH_CAST2`.
    fn aliases() -> Vec<String> {
        vec!["/cast".into(), "/spell".into()]
    }

    #[test]
    fn macro_lines_trims_blanks_and_windows_line_endings() {
        let body = "/cast Fireball\r\n\r\n  /say pew  \n";
        let lines: Vec<&str> = macro_lines(body).collect();
        assert_eq!(lines, ["/cast Fireball", "/say pew"]);
    }

    /// The aliases are the globals as they stand, to the first absent one.
    #[test]
    fn the_aliases_are_read_from_the_globals_to_the_first_gap() {
        let s = crate::script::UiScript::new().unwrap();
        s.run(r#"SLASH_CAST1 = "/cast" SLASH_CAST2 = "/spell" SLASH_CAST4 = "/x""#)
            .unwrap();
        assert_eq!(cast_aliases(s.lua()), aliases());
    }

    /// `/spell` is `SLASH_CAST2` in the shipped strings.
    #[test]
    fn cast_name_reads_the_first_cast_line_through_the_aliases() {
        let t = aliases();
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
        let t = aliases();
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
        let t = aliases();
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
        let t = aliases();
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
    fn bound(t: &[String], body: &str, book: &SpellBookState) -> i32 {
        derive(
            t,
            body,
            book,
            &PetBookState::default(),
            MacroBinding::default(),
        )
        .spell
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
        let t = aliases();
        let body = "/script CastSpellByName(\"\")\n/cast Fireball";
        assert_eq!(cast_name(&t, body), Some((CastArm::ByName, String::new())));
        assert_eq!(bound(&t, body, &fireball_book()), 0);
        // A computed first argument binds the quoted fallback after it, not the later line.
        assert_eq!(
            bound(
                &t,
                "/script CastSpellByName(x and \"Fireball\" or \"Frostbolt\")\n/cast Frostbolt",
                &fireball_book()
            ),
            133
        );
    }

    #[test]
    fn a_bound_spell_resolves_through_the_book() {
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
        let t = aliases();
        // No subtext -> the highest known rank.
        assert_eq!(bound(&t, "/cast Fireball", &book), 145);
        // A pinned subtext -> that rank. The spaced form keeps its space in the name
        // (`0x4b3950` trims nothing), so it names no spell and the bar greys the slot.
        assert_eq!(bound(&t, "/cast Fireball(Rank 1)", &book), 133);
        assert_eq!(bound(&t, "/cast Fireball (Rank 1)", &book), -1);
    }

    /// An unknown `/cast` is -1 (`0x4eff48`), which the bar greys; no cast line, or an unknown
    /// `CastSpellByName(`, is 0 (`0x4eff81`).
    #[test]
    fn an_unresolved_cast_is_arm_dependent() {
        let book = SpellBookState::default();
        let t = aliases();
        assert_eq!(bound(&t, "/cast Pyroblast", &book), -1);
        assert_eq!(bound(&t, "/spell Pyroblast\n/say pew", &book), -1);
        assert_eq!(bound(&t, "/say hi", &book), 0);
        assert_eq!(bound(&t, ".spawn 16032", &book), 0);
        assert_eq!(
            bound(&t, r#"/script CastSpellByName("Pyroblast")"#, &book),
            0
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
        let t = aliases();
        let player = SpellBookState {
            tabs: Vec::new(),
            slots: vec![named(133, "Fireball"), named(1, "Seduction")],
        };
        let pet = PetBookState {
            token: None,
            slots: vec![named(6358, "Seduction"), named(19647, "Spell Lock")],
        };
        let at = |spell, pet_book| MacroBinding { spell, pet_book };
        let derived = |body: &str| derive(&t, body, &player, &pet, MacroBinding::default());
        assert_eq!(derived("/cast Spell Lock"), at(19647, true));
        assert_eq!(
            derived(r#"/script CastSpellByName("Spell Lock")"#),
            at(19647, true),
            "the quoted form binds it too"
        );
        // The player's book is searched first.
        assert_eq!(derived("/cast Seduction"), at(1, false));
        assert_eq!(derived("/cast Fireball"), at(133, false));
        // A name in neither book is what it was: -1 for a `/cast`, 0 for the quoted form.
        assert_eq!(derived("/cast Claw"), at(-1, false));
        assert_eq!(derived(r#"/script CastSpellByName("Claw")"#), at(0, false));
        // With the pet gone, a pet spell is a name in neither.
        let gone = |was| {
            derive(
                &t,
                "/cast Spell Lock",
                &player,
                &PetBookState::default(),
                was,
            )
        };
        assert_eq!(gone(MacroBinding::default()).spell, -1);
        // A miss leaves the book selector (`0x4b3a10` writes it on a match alone), so a record
        // that was the pet's stays the pet's through it, and a cast line that is gone keeps it too.
        assert!(gone(at(19647, true)).pet_book);
        assert!(!gone(MacroBinding::default()).pet_book);
        assert_eq!(
            derive(&t, "/say hi", &player, &pet, at(19647, true)),
            at(0, true)
        );
    }

    /// `0x4f0ce0` derives a record again at a pet spell message only when `[rec+0x564] < 0` or
    /// `[rec+0x568] != 0` (`0x4f0cfb`-`0x4f0d11`), whatever it is bound to now.
    #[test]
    fn only_an_unresolved_or_pet_selected_record_follows_the_pets_list() {
        let at = |spell, pet_book| MacroBinding { spell, pet_book };
        assert!(at(-1, false).follows_the_pet_book());
        assert!(at(6358, true).follows_the_pet_book());
        assert!(
            at(0, true).follows_the_pet_book(),
            "a quoted call that lost the pet's spell keeps the pet's book"
        );
        assert!(!at(0, false).follows_the_pet_book());
        assert!(!at(133, false).follows_the_pet_book());
    }
}
