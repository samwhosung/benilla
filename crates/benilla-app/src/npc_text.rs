//! The `$`-token expansion of server-authored NPC text (gossip, questgiver panels, the quest log),
//! which the client does, not the server; every feed that pushes NPC text into the VM runs it, as
//! the reference routes its fourteen call sites through one expander (`0x506f70`, token handler
//! `0x5070a0`).
//!
//! - The accepted set is `B C E G N R T W` in either case; anything else re-emits the `$` and
//!   leaves the letter as text.
//! - A decimal prefix is consumed ahead of every token, but only `W` and `E` read it.
//! - Case is the output switch: `$r` is `$R` through `_strlwr`, and likewise `$c` and `$t`.
//!
//! Spell descriptions use a separate expander in the reference, as here
//! ([`benilla_formats::substitute`]).

use bevy::prelude::*;

use crate::names::NameCache;
use crate::net::{Guid, GuidIndex, ObjectStore, SelfPlayer};
use crate::query_cache::Ask;
use crate::ui_unit::UnitNames;

/// The unit a `$`-macro expands against: the self player, or the speaker at the reference's four
/// chat sites.
pub(crate) struct Subject {
    pub name: String,
    /// `UNIT_FIELD_BYTES_0` bytes 0 and 1 resolved through `ChrRaces` and `ChrClasses`, at the
    /// client's locale column; empty for an id with no row, which the reference faults on.
    pub race: String,
    pub class: String,
    /// `UNIT_FIELD_BYTES_0` byte 2; `$G` takes the first arm on 0, the second on anything else.
    pub gender: u8,
}

/// Everything a `$`-token can read; `states` is the reference's global world-state table
/// (`[0xb71ec8]`).
pub(crate) struct MacroContext<'a> {
    /// `None` is the reference's no-subject case.
    pub subject: Option<&'a Subject>,
    pub states: &'a crate::world_state::WorldStates,
}

/// Expands the macros in `text`. With no subject every person-token re-emits its `$`, so a name
/// not yet known shows `$N` until the feeds re-substitute.
pub(crate) fn substitute(text: &str, ctx: &MacroContext) -> String {
    substitute_checked(text, ctx).0
}

/// [`substitute`] plus `0x506f70`'s return flag, `true` when no token failed (no subject, or a
/// token outside the set). Panels ignore it; chat must not, since the reference's chat path drops
/// or defers a line rather than show a `$`.
pub(crate) fn substitute_checked(text: &str, ctx: &MacroContext) -> (String, bool) {
    let subject = ctx.subject;
    let mut clean = true;
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '$' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        // An unaccepted `$5X` loses its digits too.
        let mut j = i + 1;
        while chars.get(j).is_some_and(char::is_ascii_digit) {
            j += 1;
        }
        // On a fail the `$` comes back and the letter is copied as text.
        let mut fail = false;
        match chars.get(j).copied() {
            // Exactly one `\n`, no CR.
            Some('B' | 'b') => {
                out.push('\n');
                i = j + 1;
            }
            // The world-state table (`SMSG_INIT_WORLD_STATES`, `SMSG_UPDATE_WORLD_STATE`); `E`
            // reads the negated key; both render `%d`, and a miss prints "0".
            Some(tok @ ('W' | 'w' | 'E' | 'e')) => {
                // `SStrToInt` over the prefix. No digits reads an uninitialized buffer in the
                // reference; we take key 0, as for an id too wide for a dword.
                let n: u32 = chars[i + 1..j]
                    .iter()
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0);
                let key = if matches!(tok, 'E' | 'e') {
                    n.wrapping_neg()
                } else {
                    n
                };
                out.push_str(&ctx.states.get(key).to_string());
                i = j + 1;
            }
            Some('N' | 'n') => match subject {
                Some(s) => {
                    out.push_str(&s.name);
                    i = j + 1;
                }
                None => fail = true,
            },
            Some(tok @ ('R' | 'r' | 'C' | 'c')) => match subject {
                Some(s) => {
                    // An id outside the table faults the reference; we emit nothing.
                    let name = if matches!(tok, 'R' | 'r') {
                        s.race.as_str()
                    } else {
                        s.class.as_str()
                    };
                    if tok.is_ascii_lowercase() {
                        out.push_str(&name.to_ascii_lowercase());
                    } else {
                        out.push_str(name);
                    }
                    i = j + 1;
                }
                None => fail = true,
            },
            // `$T` is the PvP rank title, falling back to this gender branch, never lower-cased,
            // when the `PVP_RANK_<rank>_<team>` GlobalString misses. We ship no rank titles, so it
            // always misses.
            Some('G' | 'g' | 'T' | 't') => match subject {
                Some(s) => {
                    let mut k = j + 1;
                    while chars.get(k) == Some(&' ') {
                        k += 1;
                    }
                    match parse_branch(&chars, k) {
                        Some((first, second, end)) => {
                            out.extend(if s.gender == 0 { first } else { second });
                            i = end;
                        }
                        // Malformed: the marker and its spaces go, the argument stays as text.
                        None => i = k,
                    }
                }
                None => fail = true,
            },
            // Not in `B C E G N R T W`, or the string ended on the `$`.
            _ => fail = true,
        }
        if fail {
            out.push('$');
            i = j;
            clean = false;
        }
    }
    (out, clean)
}

/// Whether `text`'s expansion fails with no subject: it holds a person token, or one outside the
/// set. Only such a failure makes the chat path wait on the subject's name (`0x49dac2`).
pub(crate) fn needs_subject(text: &str) -> bool {
    let states = crate::world_state::WorldStates::default();
    let ctx = MacroContext {
        subject: None,
        states: &states,
    };
    !substitute_checked(text, &ctx).1
}

/// Parses a `first:second;` branch body: the two arms and the index past the `;`.
fn parse_branch(chars: &[char], start: usize) -> Option<(&[char], &[char], usize)> {
    let colon = (start..chars.len()).find(|&j| chars[j] == ':' || chars[j] == ';')?;
    if chars[colon] != ':' {
        return None;
    }
    let semi = (colon + 1..chars.len()).find(|&j| chars[j] == ';')?;
    Some((
        trim_spaces(&chars[start..colon]),
        trim_spaces(&chars[colon + 1..semi]),
        semi + 1,
    ))
}

/// Trims leading and trailing `' '` only, as the reference's branch copy does.
fn trim_spaces(arm: &[char]) -> &[char] {
    let start = arm.iter().position(|&c| c != ' ').unwrap_or(arm.len());
    let end = arm.iter().rposition(|&c| c != ' ').map_or(start, |p| p + 1);
    &arm[start..end]
}

/// The self player as a macro [`Subject`], once it is streamed and its name is known. The expander
/// reads the name cache without a callback (`0x506fd4`), and our own name is seeded at login.
pub(crate) fn player_identity(
    self_q: &Query<(&ObjectStore, &Guid), With<SelfPlayer>>,
    names: &NameCache,
    unit_names: UnitNames<'_>,
) -> Option<Subject> {
    let (store, guid) = self_q.iter().next()?;
    Some(Subject {
        name: names.resolve_player(guid.0, Ask::Read)?.to_string(),
        race: race_word(unit_names, store.0.unit_race().unwrap_or(0)),
        class: class_word(unit_names, store.0.unit_class().unwrap_or(0)),
        gender: store.0.unit_gender().unwrap_or(0),
    })
}

/// A race byte's display name, empty with no row.
fn race_word(names: UnitNames<'_>, race: u8) -> String {
    names
        .race(race)
        .map_or_else(String::new, |(n, _)| n.to_string())
}

/// A class byte's display name, empty with no row.
fn class_word(names: UnitNames<'_>, class: u8) -> String {
    names
        .class(class)
        .map_or_else(String::new, |(n, _)| n.to_string())
}

/// The chat feed's [`Subject`] for any guid: the object manager's descriptors, else the name
/// cache (`0x506f70`), which the expansion only reads (`0x506fd4`); the chat handler's own lookup
/// with a callback asks for it (`0x49d9f5`), as `ask` says. Guid 0 has no subject.
pub(crate) fn subject_for_guid(
    guid: u64,
    index: &GuidIndex,
    stores: &Query<&ObjectStore>,
    names: &NameCache,
    ask: Ask<'_>,
    unit_names: UnitNames<'_>,
) -> Option<Subject> {
    if guid == 0 {
        return None;
    }
    let store = index.0.get(&guid).and_then(|e| stores.get(*e).ok());
    let name = names.resolve_unit(guid, store, ask)?.to_string();
    if let Some(store) = store {
        return Some(Subject {
            name,
            race: race_word(unit_names, store.0.unit_race().unwrap_or(0)),
            class: class_word(unit_names, store.0.unit_class().unwrap_or(0)),
            gender: store.0.unit_gender().unwrap_or(0),
        });
    }
    // A creature lands on zeros, so `$R` and `$C` emit nothing; the reference's non-player arm
    // emits the unit's name (`0x50716b`, `0x5071f7`), which is not built.
    let (race, class, gender) = names.player_traits(guid).unwrap_or((0, 0, 0));
    Some(Subject {
        name,
        race: race_word(unit_names, race),
        class: class_word(unit_names, class),
        gender,
    })
}

#[cfg(test)]
mod tests {
    use super::{substitute, substitute_checked, MacroContext, Subject};
    use crate::ui_unit::TestNames;
    use crate::world_state::WorldStates;

    /// Race 4, class 5: both real rows, named by the table at the install's locale.
    fn subject_in(names: &TestNames, gender: u8) -> Subject {
        Subject {
            name: "Thrall".into(),
            race: super::race_word(names.names(), 4),
            class: super::class_word(names.names(), 5),
            gender,
        }
    }

    fn subject(gender: u8) -> Subject {
        subject_in(&TestNames::english(), gender)
    }

    fn expand(text: &str, subject: Option<&Subject>) -> String {
        substitute(
            text,
            &MacroContext {
                subject,
                states: &WorldStates::default(),
            },
        )
    }

    #[test]
    fn the_return_flag_reports_whether_any_token_failed() {
        let s = subject(0);
        let states = WorldStates::default();
        let ctx = |subject| MacroContext {
            subject,
            states: &states,
        };

        assert_eq!(
            substitute_checked("plain text", &ctx(None)),
            ("plain text".to_string(), true)
        );
        assert_eq!(
            substitute_checked("hi $N", &ctx(Some(&s))),
            ("hi Thrall".to_string(), true)
        );
        assert_eq!(
            substitute_checked("hi $N", &ctx(None)),
            ("hi $N".to_string(), false)
        );
        // Outside the set: fails even with a subject.
        assert_eq!(
            substitute_checked("hi $X", &ctx(Some(&s))),
            ("hi $X".to_string(), false)
        );
        // One failure anywhere poisons the flag, though the rest still expands.
        assert_eq!(
            substitute_checked("$N and $X", &ctx(Some(&s))),
            ("Thrall and $X".to_string(), false)
        );
    }

    #[test]
    fn name_and_newline() {
        let s = subject(0);
        assert_eq!(expand("Greetings $N", Some(&s)), "Greetings Thrall");
        assert_eq!(expand("Greetings $n", Some(&s)), "Greetings Thrall");
        assert_eq!(expand("Hail,$Bfriend", Some(&s)), "Hail,\nfriend");
    }

    #[test]
    fn race_and_class_follow_the_token_case() {
        let s = subject(0);
        assert_eq!(expand("A $C of $R", Some(&s)), "A Priest of Night Elf");
        assert_eq!(expand("a $c of $r", Some(&s)), "a priest of night elf");
        let unknown = Subject {
            class: super::class_word(TestNames::english().names(), 6),
            ..subject(0)
        };
        assert_eq!(expand("[$C]", Some(&unknown)), "[]");
    }

    /// `$R` and `$C` are the table's words at the install's locale, not English.
    #[test]
    fn race_and_class_are_the_installs_words() {
        let s = subject_in(&TestNames::korean(), 0);
        assert_eq!(expand("$C / $R", Some(&s)), "사제 / 나이트 엘프");
    }

    #[test]
    fn gender_branches_on_zero() {
        assert_eq!(
            expand("Well met, $Glad:lass;.", Some(&subject(0))),
            "Well met, lad."
        );
        assert_eq!(
            expand("Well met, $Glad:lass;.", Some(&subject(1))),
            "Well met, lass."
        );
        assert_eq!(expand("$Glad:lass;", Some(&subject(2))), "lass");
        assert_eq!(
            expand("Well met, $G lad : lass ;.", Some(&subject(1))),
            "Well met, lass."
        );
        assert_eq!(expand("$tLad:Lass;", Some(&subject(0))), "Lad");
    }

    #[test]
    fn malformed_branch_drops_the_marker_not_the_text() {
        let s = subject(0);
        assert_eq!(expand("Broken $Gbranch", Some(&s)), "Broken branch");
        assert_eq!(expand("$G male female;", Some(&s)), "male female;");
        assert_eq!(expand("$G male:female", Some(&s)), "male:female");
    }

    #[test]
    fn world_state_tokens_read_an_empty_table() {
        let s = subject(0);
        assert_eq!(expand("$2077w gathered", Some(&s)), "0 gathered");
        assert_eq!(expand("$2077e gathered", Some(&s)), "0 gathered");
        // A bare `$w` is key 0.
        assert_eq!(expand("$w", Some(&s)), "0");
    }

    #[test]
    fn world_state_tokens_render_received_values() {
        let s = subject(0);
        let mut states = WorldStates::default();
        states.write(&[
            (2077, 12),
            (2077u32.wrapping_neg(), 3),
            (2264, -5i32 as u32),
        ]);
        let filled = |text: &str| {
            substitute(
                text,
                &MacroContext {
                    subject: Some(&s),
                    states: &states,
                },
            )
        };
        assert_eq!(filled("$2077w gathered"), "12 gathered");
        assert_eq!(filled("$2077e gathered"), "3 gathered");
        assert_eq!(filled("$2264w"), "-5", "rendered %d, not %u");
        assert_eq!(filled("$9999w"), "0", "an id the zone never sent");
        // `W` and `w` are one token.
        assert_eq!(filled("$2077W"), "12");
    }

    #[test]
    fn unaccepted_tokens_keep_the_dollar() {
        let s = subject(0);
        assert_eq!(expand("A $X here", Some(&s)), "A $X here");
        assert_eq!(expand("A $5X here", Some(&s)), "A $X here");
        assert_eq!(expand("Cost: 5$", Some(&s)), "Cost: 5$");
    }

    #[test]
    fn no_subject_leaves_the_text_literal() {
        assert_eq!(expand("Greetings $N", None), "Greetings $N");
        assert_eq!(expand("$Glad:lass; $C", None), "$Glad:lass; $C");
        assert_eq!(expand("Hail,$Bfriend $w", None), "Hail,\nfriend 0");
    }
}
