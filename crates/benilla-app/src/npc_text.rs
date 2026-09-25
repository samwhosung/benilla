//! NPC-text macros — the client-side `$`-token expansion the 1.12 wire leaves undone in every
//! server-authored text the player reads: gossip greetings, questgiver panel texts, and the quest
//! log's description/objectives (the director's screenshot showed a literal `$N` in a quest
//! description). One shared mechanism (the 0109 look fix promoted it out of `ui_gossip`); every feed
//! seam that pushes NPC text into the VM runs it — as the reference does, routing all fourteen of
//! its call sites through the one expander.
//!
//! The grammar is the reference's (`QuestTextParser.cpp`, driver `0x506f70` → token handler
//! `0x5070a0`):
//!
//! - the accepted set is **exactly** `B C E G N R T W`, in either case. Anything else re-emits the
//!   `$` and lets the letter fall through as ordinary literal text;
//! - an optional decimal prefix is scanned and consumed ahead of *every* token, but only `W`/`E`
//!   read it;
//! - **case is the output switch, not a separate token**: `$R` is the race string verbatim
//!   (`"Night Elf"`), `$r` is that same string through `_strlwr` (`"night elf"`). Same for
//!   `$C`/`$c` and `$T`/`$t`; `$N`/`$B`/`$G` are case-insensitive.
//!
//! The reference's *other* `$`-expander — spell descriptions, with its `$/N;` scale prefixes and
//! `$<spellId>` cross-references — is a different function over a different source. Ours is
//! [`benilla_formats::substitute`], kept separate here exactly as it is there.

use bevy::prelude::*;

use crate::names::NameCache;
use crate::net::{Guid, GuidIndex, NetCommands, ObjectStore, SelfPlayer};

/// The unit a `$`-macro expands against. Every seam we have passes the **self player** (the
/// reference resolves the subject from the GUID its call site hands over, and only its four chat
/// sites pass the speaker instead), so the reference's non-player arm — where `$R`/`$C` emit the
/// unit's *name* in place of a race/class — is not reachable from here.
pub(crate) struct Subject {
    pub name: String,
    /// `UNIT_FIELD_BYTES_0` byte 0 / byte 1 — indices into the `ChrRaces`/`ChrClasses` tables the
    /// reference reads by locale column. Ours are [`crate::ui_unit`]'s hardcoded English rows, which
    /// were checked character-for-character against the shipped DBCs.
    pub race: u8,
    pub class: u8,
    /// `UNIT_FIELD_BYTES_0` byte 2. The `$G`/`$T` branch test is `== 0` → the *first* arm, anything
    /// else → the second — the reference compares against zero, so this is not "1 = female".
    pub gender: u8,
}

/// Everything a `$`-token can read: the [`Subject`] the person-tokens expand against, and the
/// world-state table `$<n>w`/`$<n>e` index. The reference reads the latter from a process global
/// (`[0xb71ec8]`); we hold it as a resource, so the expander takes it explicitly — the same shape
/// the sibling spell expander already uses (`benilla_formats::TokenContext`).
pub(crate) struct MacroContext<'a> {
    /// `None` is the reference's no-subject case — see [`substitute`].
    pub subject: Option<&'a Subject>,
    pub states: &'a crate::world_state::WorldStates,
}

/// Substitute the macros in `text` against `ctx` (see the module doc for the grammar). A `None`
/// subject is the reference's no-subject case: every token that needs one fails, which re-emits the
/// `$` and leaves the rest of the text literal — so an un-landed player name shows `$N` for the
/// moment rather than a hole, and the feeds re-substitute when it arrives.
pub(crate) fn substitute(text: &str, ctx: &MacroContext) -> String {
    substitute_checked(text, ctx).0
}

/// [`substitute`], plus the reference driver's **return flag**: `true` when no token failed.
///
/// The reference's `0x506f70` returns exactly this — "no unrecognized token was hit" — and its
/// callers branch on it. The panel seams ignore it (they show the `$`-preserving text either way,
/// which is what [`substitute`] hands back); the **chat** seam must not, because the reference's
/// chat path never displays a `$` — it drops or defers the line instead. See
/// `ui_chat::feed`'s use.
///
/// Note what the flag does NOT mean: it says nothing about truncation, and a `false` can come
/// either from an unresolvable subject or from a token outside the accepted set.
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
        // The decimal prefix is consumed ahead of the token letter whatever the token turns out to
        // be — so `$5N` is just `$N`, and an unaccepted `$5X` loses the digits on its way out.
        let mut j = i + 1;
        while chars.get(j).is_some_and(char::is_ascii_digit) {
            j += 1;
        }
        // `fail` is the reference's `false` return: the `$` comes back and the cursor stays on the
        // letter, which the literal path then copies.
        let mut fail = false;
        match chars.get(j).copied() {
            // Exactly one `\n` (no CR) — the text renderer splits on it. Needs no subject.
            Some('B' | 'b') => {
                out.push('\n');
                i = j + 1;
            }
            // `$<n>W` / `$<n>E` — the world-state table ([`crate::world_state`]), filled by
            // SMSG_INIT_WORLD_STATES (`0x2C2`) and SMSG_UPDATE_WORLD_STATE (`0x2C3`); `$…E` reads
            // that same table at the *negated* key, and both render `%d`. A miss prints `"0"` —
            // which is every lookup until a zone actually sends states, and is what the reference
            // prints then too.
            Some(tok @ ('W' | 'w' | 'E' | 'e')) => {
                // `SStrToInt` over the prefix. No digits at all reads an uninitialized buffer in
                // the reference (undefined); we take it as key 0, and an id too wide for the
                // dword the wire carries goes the same way rather than wrapping to a live key.
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
                    // An id outside the table is where the reference dereferences near-NULL and
                    // faults outright; we emit nothing.
                    let name = if matches!(tok, 'R' | 'r') {
                        crate::ui_unit::race_names(s.race).map_or("", |(display, _)| display)
                    } else {
                        crate::ui_unit::class_names(s.class).map_or("", |(display, _)| display)
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
            // `$T`/`$t` is the PvP rank title, with this same gender branch as its fallback when the
            // `PVP_RANK_<rank>_<team>` GlobalString misses — which it always does here, since we
            // ship no rank titles. So the two tokens share one path, and `$t` lower-cases nothing:
            // the reference never lower-cases the fallback text either.
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
                        // A malformed branch swallows the marker and the spaces after it and emits
                        // nothing at all — its argument survives as plain text, so `$G male female;`
                        // renders `male female;`. (We used to pass the whole token through; the
                        // bytes say the marker is consumed.)
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

/// Parse a `$G`/`$T` branch body `first:second;` starting at `start` (already past the marker and
/// the spaces behind it), returning the two arms and the index just past the terminating `;`.
/// `None` — malformed — when there is no `:` before the `;`, or no `;` at all.
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

/// Drop the spaces the reference's branch copy drops: the leading run (after the marker and after
/// the `:`) and the trailing one. Only `' '` — its skip loops test that byte, not whitespace.
fn trim_spaces(arm: &[char]) -> &[char] {
    let start = arm.iter().position(|&c| c != ' ').unwrap_or(arm.len());
    let end = arm.iter().rposition(|&c| c != ' ').map_or(start, |p| p + 1);
    &arm[start..end]
}

/// The self player as a macro [`Subject`] — the name from the [`NameCache`] (a miss queries the
/// server once, like the unit frames), race/class/gender from the descriptor. `None` until the
/// player is streamed *and* its name has landed; the feeds diff on the substituted text, so it
/// re-substitutes when that happens.
pub(crate) fn player_identity(
    self_q: &Query<(&ObjectStore, &Guid), With<SelfPlayer>>,
    names: &NameCache,
    commands: &NetCommands,
) -> Option<Subject> {
    let (store, guid) = self_q.iter().next()?;
    Some(Subject {
        name: names.resolve(guid.0, commands)?.to_string(),
        race: store.0.unit_race().unwrap_or(0),
        class: store.0.unit_class().unwrap_or(0),
        gender: store.0.unit_gender().unwrap_or(0),
    })
}

/// A macro [`Subject`] for an **arbitrary** guid — the chat feed's subject, where every other seam
/// passes the local player.
///
/// This is the reference's own two-step (`0x506f70`): look the guid up in the
/// object manager first and read the unit's descriptors, and only when it isn't streamed fall back
/// to the **name-cache** record. `None` means the subject could not be resolved at all — the
/// reference's no-subject case, which fails every person-token and re-emits a literal `$`; that is
/// also what an untargeted line (guid 0) gets, deliberately.
pub(crate) fn subject_for_guid(
    guid: u64,
    index: &GuidIndex,
    stores: &Query<&ObjectStore>,
    names: &NameCache,
    commands: &NetCommands,
) -> Option<Subject> {
    if guid == 0 {
        return None;
    }
    let store = index.0.get(&guid).and_then(|e| stores.get(*e).ok());
    let name = names.resolve_unit(guid, store, commands)?.to_string();
    if let Some(store) = store {
        return Some(Subject {
            name,
            race: store.0.unit_race().unwrap_or(0),
            class: store.0.unit_class().unwrap_or(0),
            gender: store.0.unit_gender().unwrap_or(0),
        });
    }
    // Not streamed: the name answer's own race/class/gender. A creature guid has no such record and
    // lands on zeros — which is right, because the reference's non-player arm never reads a
    // race/class for `$R`/`$C` either; it emits the unit's name instead (`0x50716b`/`0x5071f7`).
    let (race, class, gender) = names.player_traits(guid).unwrap_or((0, 0, 0));
    Some(Subject {
        name,
        race,
        class,
        gender,
    })
}

#[cfg(test)]
mod tests {
    use super::{substitute, substitute_checked, MacroContext, Subject};
    use crate::world_state::WorldStates;

    /// Thrall the night-elf priest — race 4 / class 5, so both table lookups are real rows.
    fn subject(gender: u8) -> Subject {
        Subject {
            name: "Thrall".into(),
            race: 4,
            class: 5,
            gender,
        }
    }

    /// Expand against a subject and an empty world-state table — the shape of every test whose
    /// concern is a person-token.
    fn expand(text: &str, subject: Option<&Subject>) -> String {
        substitute(
            text,
            &MacroContext {
                subject,
                states: &WorldStates::default(),
            },
        )
    }

    /// The driver's return flag ([`substitute_checked`]) — `true` only when no token failed. The
    /// chat seam branches on it (drop / defer / show raw), so a wrong flag silently loses chat lines
    /// rather than merely showing a stray `$`.
    #[test]
    fn the_return_flag_reports_whether_any_token_failed() {
        let s = subject(0);
        let states = WorldStates::default();
        let ctx = |subject| MacroContext {
            subject,
            states: &states,
        };

        // No `$` at all, and a token that resolves: clean.
        assert_eq!(
            substitute_checked("plain text", &ctx(None)),
            ("plain text".to_string(), true)
        );
        assert_eq!(
            substitute_checked("hi $N", &ctx(Some(&s))),
            ("hi Thrall".to_string(), true)
        );
        // No subject: the person-token fails, the `$` comes back, flag false.
        assert_eq!(
            substitute_checked("hi $N", &ctx(None)),
            ("hi $N".to_string(), false)
        );
        // Outside the accepted set: fails even WITH a subject — the case the chat path drops
        // outright rather than deferring, because no name query can fix it.
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
        // An id outside the table emits nothing (where the reference faults).
        let unknown = Subject {
            class: 6,
            ..subject(0)
        };
        assert_eq!(expand("[$C]", Some(&unknown)), "[]");
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
        // Not "1 = female": zero picks the first arm and everything else the second.
        assert_eq!(expand("$Glad:lass;", Some(&subject(2))), "lass");
        // The spaces after the marker, after the `:`, and at each arm's end are dropped.
        assert_eq!(
            expand("Well met, $G lad : lass ;.", Some(&subject(1))),
            "Well met, lass."
        );
        // `$T` has no rank title to find, so it falls back to this same branch, un-lower-cased.
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
        // A bare `$w` is key 0 — a miss like any other.
        assert_eq!(expand("$w", Some(&s)), "0");
    }

    /// The point of the table: once a zone's states land, `$<n>w` renders the value rather than the
    /// standing `"0"`. `$<n>e` reads the SAME table at the negated key, so the two tokens with the
    /// same digits are different lookups — and a value is rendered `%d`, sign and all.
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
        // Case is not the output switch here — unlike `$R`/`$C`, `W` and `w` are one token.
        assert_eq!(filled("$2077W"), "12");
    }

    #[test]
    fn unaccepted_tokens_keep_the_dollar() {
        let s = subject(0);
        assert_eq!(expand("A $X here", Some(&s)), "A $X here");
        // The digits are consumed before the letter is judged, so they are lost with it.
        assert_eq!(expand("A $5X here", Some(&s)), "A $X here");
        assert_eq!(expand("Cost: 5$", Some(&s)), "Cost: 5$");
    }

    #[test]
    fn no_subject_leaves_the_text_literal() {
        assert_eq!(expand("Greetings $N", None), "Greetings $N");
        assert_eq!(expand("$Glad:lass; $C", None), "$Glad:lass; $C");
        // `$B` and the world-state tokens need no subject.
        assert_eq!(expand("Hail,$Bfriend $w", None), "Hail,\nfriend 0");
    }
}
