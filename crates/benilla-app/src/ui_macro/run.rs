//! A macro body's runnable lines, and the spell it binds for the action bar.
//!
//! A macro slot shows the macro's icon but the cooldown, usability, range and checked state of its
//! bound spell, the record's `[rec+0x564]`, which the slot resolver `0x4e5a50` returns for a
//! macro. `0x4efe00` derives it from the first line that is a `/cast` alias or holds
//! `CastSpellByName(`, through the resolver the `CastSpellByName` binding (`0x4b4ab0`) uses
//! (`0x4b3950` → `0x4b3a10`), so a bare name binds the highest known rank.
//!
//! `[rec+0x568]` is the book the spell resolved from, 0 the player's and 1 the pet's. benilla
//! resolves against the player's book only, so a macro that casts a pet spell binds nothing.

use benilla_ui::script::{resolve_spell_by_name, SpellBookState};

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
/// `CastSpellByName(` with a quoted argument. Deviation: the `/cast` argument is trimmed, because
/// the reference binds `/cast  Fireball` (two spaces) to a name that never resolves.
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
        // Arm B, second, on the same line (`0x4efed5`).
        if let Some(name) = quoted_call_argument(line) {
            return Some((CastArm::ByName, name));
        }
    }
    None
}

/// `… CastSpellByName("Fireball" …) …` gives `Fireball`: only a non-empty, double-quoted first
/// argument is read. The reference reads from the first quote anywhere after the call (`0x4efef0`)
/// and binds even an empty name, which ends its walk on that line.
fn quoted_call_argument(line: &str) -> Option<String> {
    let after = line.find(CAST_BY_NAME_CALL)? + CAST_BY_NAME_CALL.len();
    let rest = line.get(after..)?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let close = rest.find('"')?;
    let name = &rest[..close];
    (!name.is_empty()).then(|| name.to_string())
}

/// The macro's bound spell: [`cast_name`] resolved as `CastSpellByName` resolves
/// ([`resolve_spell_by_name`]), so the cooldown swirl and the press agree on the rank.
pub(crate) fn bound_spell(table: &SlashCommands, body: &str, book: &SpellBookState) -> BoundSpell {
    let Some((arm, name)) = cast_name(table, body) else {
        return BoundSpell::None;
    };
    match (resolve_spell_by_name(book, &name), arm) {
        (Some(s), _) => BoundSpell::Spell(s.spell_id),
        (None, CastArm::Slash) => BoundSpell::Unresolved,
        (None, CastArm::ByName) => BoundSpell::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_chat::commands::SlashCommands;

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
        // A computed argument has no name to bind.
        assert_eq!(cast_name(&t, "/script CastSpellByName(spell)"), None);
        assert_eq!(cast_name(&t, r#"/script CastSpellByName("")"#), None);
    }

    #[test]
    fn bound_spell_resolves_through_the_book() {
        use benilla_ui::script::SpellSlotView;

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
        assert_eq!(
            bound_spell(&t, "/cast Fireball", &book),
            BoundSpell::Spell(145)
        );
        // A pinned subtext -> that rank, both spacings.
        assert_eq!(
            bound_spell(&t, "/cast Fireball(Rank 1)", &book),
            BoundSpell::Spell(133)
        );
        assert_eq!(
            bound_spell(&t, "/cast Fireball (Rank 1)", &book),
            BoundSpell::Spell(133)
        );
    }

    /// An unknown `/cast` is -1 (`0x4eff48`), which the bar greys; no cast line, or an unknown
    /// `CastSpellByName(`, is 0 (`0x4eff81`).
    #[test]
    fn an_unresolved_cast_is_arm_dependent() {
        let book = SpellBookState::default();
        let t = table();
        assert_eq!(
            bound_spell(&t, "/cast Pyroblast", &book),
            BoundSpell::Unresolved
        );
        assert_eq!(
            bound_spell(&t, "/spell Pyroblast\n/say pew", &book),
            BoundSpell::Unresolved
        );
        assert_eq!(bound_spell(&t, "/say hi", &book), BoundSpell::None);
        assert_eq!(bound_spell(&t, ".spawn 16032", &book), BoundSpell::None);
        assert_eq!(
            bound_spell(&t, r#"/script CastSpellByName("Pyroblast")"#, &book),
            BoundSpell::None
        );
    }
}
