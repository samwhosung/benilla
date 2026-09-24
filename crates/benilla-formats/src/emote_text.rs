//! `EmotesText.dbc` × `EmotesTextData.dbc` — the **sentence** an `SMSG_TEXT_EMOTE` becomes
//! ("Bob waves at you." / "You wave." / "Bob waves at Jane."), decision 1274.
//!
//! The sibling module [`crate::emotes`] owns the same `EmotesText.dbc`'s *other* half — the
//! `/command` name, the anim id, the voice kits. This one owns its 16 **`EmoteText[]`** columns and
//! the localized-string table they point into, because the two are consumed by different arcs
//! (audio+animation vs the chat log) and only one of them needs `EmotesTextData.dbc` at all.
//!
//! The law below is the real client's sole sentence composer, **`0x49b200`** — read at the bytes
//! for this implementation (`objdump 0x49b200..0x49b47c`), which is where the byte addresses in
//! the comments come from.
//!
//! # The composer, in three parts
//!
//! **1 · A 4-bit column selector.** `index = gender | context`, where `context` is built from three
//! facts and the *order of the tests matters* (`0x49b2c3`-`0x49b316`):
//!
//! | bit | test | meaning |
//! |---|---|---|
//! | `2` (`0x49b2c8`) | performer guid == the active player's | **you are the performer** — and this *skips* the bit-0 test |
//! | `1` (`0x49b2f3`) | the wire's target name == your own name | **you are the target** (only ever tested when you are not the performer) |
//! | `4` (`0x49b311`) | the wire's target name is empty | **untargeted** |
//! | `8` (`0x49b31c`) | the performer's `NameCache` record `+0x13c == 1` (Female) | performer **gender** |
//!
//! Reachable columns are `{0,1,2,4,6} | gender` — 10 of the 16; 3/5/7/11/13/15 cannot be selected
//! (bit 0 and bit 2 are mutually exclusive, and bit 1 skips bit 0's test). WAVE (id 101) in the
//! shipped table: col0 "%s waves at %s.", col1 "%s waves at you.", col2 "You wave at %s.",
//! col4 "%s waves.", col6 "You wave.", every other column 0.
//!
//! **2 · A four-rung fallback ladder** when the picked column has no non-empty string for the
//! locale (`0x49b332`, `0x49b35e`, `0x49b38a`, `0x49b3c5`): `gender|ctx` → `ctx` → `gender|ctx'` →
//! `ctx'`, where `ctx' = (ctx & !1) | 4` ("drop target-self, force untargeted"). All four empty ⇒
//! **no chat line at all** (`0x49b4bd`), and the emote is anim + voice only.
//!
//! **3 · Four `SStrPrintf` sites**, picked by (performer-is-other) × (target-is-other), always
//! performer-first (`0x49b3fa`-`0x49b479`). See [`EmoteLine`] for the self-target edge that falls
//! out of the test order.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const EMOTES_TEXT: &str = "DBFilesClient\\EmotesText.dbc";
const EMOTES_TEXT_DATA: &str = "DBFilesClient\\EmotesTextData.dbc";

/// `EmotesText.dbc`'s `EmoteText[]` width — the selector is 4 bits wide because this is 16.
const FORMS: usize = 16;

/// How many locale columns an `EmotesTextData.dbc` row carries in 5875 (`id + 8 strings + flags`
/// = the 10 fields the loader's `cmp eax,0xa` at `0x544579` insists on). Same shape as
/// [`crate::languages`]'s `Name_lang` block, and read the same way: the loader keeps every column,
/// the app asks for the slot it wants (`[0xc0e080]`, 0 = enUS, the only one this install fills).
const LOCALES: usize = 8;

/// The joined text-emote sentence tables.
#[derive(Debug, Default, Clone)]
pub struct EmoteTextCatalog {
    /// `EmotesText.dbc` id → its 16 `EmoteText[]` columns (each an `EmotesTextData` id, 0 = none).
    forms: HashMap<u32, [u32; FORMS]>,
    /// `EmotesTextData.dbc` id → its localized format strings, one per locale column.
    data: HashMap<u32, [Option<String>; LOCALES]>,
}

/// The facts `0x49b200` branches on, in the composer's own terms.
///
/// **`target_is_you` is only tested when you are not the performer** — the guid compare at
/// `0x49b2c8` jumps past it — so [`EmoteTextCatalog::compose`] derives both facts here rather than
/// trusting a caller to pre-clear one.
///
/// That branch order once looked like it meant "a self-emote renders *You wave at ⟨YourName⟩.*",
/// and decision 1274 said so. **It does not** (corrected by 1282): the sending client zeroes a
/// self-target before the packet is built (`DoEmote 0x5ef611`), so `target` arrives EMPTY for that
/// action and the untargeted column wins — "You wave.". Vanilla has no reflexive form *and* no
/// self-named one; the case simply never reaches this struct.
#[derive(Debug, Clone, Copy)]
pub struct EmoteLine<'a> {
    /// The performer's resolved name — the `%s` the reference fills from the `NameCache` record
    /// pointer it passes as-is (`record+0` *is* the name).
    pub performer: &'a str,
    /// The performer guid == the active player's (`0x49b2c8`).
    pub performer_is_you: bool,
    /// The performer's sex is Female (`record+0x13c == 1`) — the `+8` on the column index.
    pub performer_female: bool,
    /// The target's display name **as the server sent it** (the wire string, never round-tripped
    /// through a name cache); empty = untargeted.
    pub target: &'a str,
    /// Your own name (`GetOwnName 0x609210`), compared case-insensitively against `target`
    /// (`0x64a480`).
    pub your_name: &'a str,
}

impl EmoteTextCatalog {
    /// The composed chat line for a text emote, or `None` when the ladder runs dry (the
    /// reference's "no line at all" tail — the emote is still an animation and a voice).
    ///
    /// `locale` is the client's locale column; 0 (enUS) is the only one a 5875 enUS install fills.
    pub fn compose(&self, text_id: u32, line: &EmoteLine, locale: usize) -> Option<String> {
        let forms = self.forms.get(&text_id)?;

        // ── the selector (`0x49b2c3`-`0x49b316`) ────────────────────────────────────────────────
        // Both flags start "the other guy" and are cleared by the branch that fires — the order is
        // load-bearing: matching the performer skips the target compare entirely.
        let (mut performer_is_other, mut target_is_other) = (true, true);
        let mut ctx = 0usize;
        if line.performer_is_you {
            ctx = 2;
            performer_is_other = false;
        } else if !line.target.is_empty() && line.target.eq_ignore_ascii_case(line.your_name) {
            ctx = 1;
            target_is_other = false;
        }
        if line.target.is_empty() {
            ctx |= 4;
        }
        let gender = usize::from(line.performer_female) * 8;

        // ── the ladder (`0x49b332` / `0x49b35e` / `0x49b38a` / `0x49b3c5`) ──────────────────────
        // `ctx` is mutated in place by rung 3, exactly as the reference mutates `esi`, so rung 4
        // reads the *corrected* context and not the original one.
        let template = self
            .template(forms, gender | ctx, locale)
            .or_else(|| self.template(forms, ctx, locale))
            .or_else(|| {
                ctx = (ctx & !1) | 4;
                self.template(forms, gender | ctx, locale)
            })
            .or_else(|| self.template(forms, ctx, locale))?;

        // ── the four printf sites (`0x49b3fa`-`0x49b479`), performer first ──────────────────────
        Some(match (performer_is_other, target_is_other) {
            (true, true) => fill(template, &[line.performer, line.target]),
            (true, false) => fill(template, &[line.performer]),
            (false, true) => fill(template, &[line.target]),
            // Provably unreachable in the reference (clearing `target_is_other` requires the
            // performer branch not to have fired); kept as the format's own no-arg case.
            (false, false) => fill(template, &[]),
        })
    }

    /// One `EmoteText[index]` column resolved to a **non-empty** localized string, or `None` — the
    /// reference's four rejections rolled into one (`index` past the array, the column's id absent
    /// from `EmotesTextData`, the row's string blank for this locale).
    fn template<'a>(
        &'a self,
        forms: &[u32; FORMS],
        index: usize,
        locale: usize,
    ) -> Option<&'a str> {
        let data_id = *forms.get(index)?;
        self.data.get(&data_id)?.get(locale)?.as_deref()
    }

    /// How many `EmotesText.dbc` rows carry a form table (169 in 5875).
    pub fn len(&self) -> usize {
        self.forms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.forms.is_empty()
    }
}

/// `SStrPrintf` (`0x64a7f0`) reduced to what these templates actually use: replace each `%s` with
/// the next argument, left to right. A template with more `%s` than arguments keeps the surplus
/// verbatim — the reference would print stack garbage there, and no shipped row does it.
fn fill(template: &str, args: &[&str]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    let mut args = args.iter();
    while let Some(at) = rest.find("%s") {
        let Some(arg) = args.next() else { break };
        out.push_str(&rest[..at]);
        out.push_str(arg);
        rest = &rest[at + 2..];
    }
    out.push_str(rest);
    out
}

/// `EmotesText.dbc` — `ID`, `Name`(str), `EmoteID`, `EmoteText[16]`; 19 fields × 4 = the 76-byte
/// record the client's loader checks for.
fn emotes_text_schema() -> Schema {
    let mut s = Schema::new("EmotesText");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("Name", FieldType::String));
    s.add_field(SchemaField::new("EmoteID", FieldType::UInt32));
    for i in 0..FORMS {
        s.add_field(SchemaField::new(format!("EmoteText{i}"), FieldType::UInt32));
    }
    s
}

/// `EmotesTextData.dbc` — `ID`, eight `Text_lang` strings, `Flags`; the classic 1.12
/// `LocalizedString` block (10 fields × 4 = 40 bytes, the reader's own `cmp eax,0x28`).
fn emotes_text_data_schema() -> Schema {
    let mut s = Schema::new("EmotesTextData");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    for i in 0..LOCALES {
        s.add_field(SchemaField::new(format!("Text{i}"), FieldType::String));
    }
    s.add_field(SchemaField::new("Flags", FieldType::UInt32));
    s
}

/// Read and join both tables off the patch chain.
pub fn load_emote_text_catalog(chain: &mut Chain) -> Result<EmoteTextCatalog> {
    let bytes = chain
        .read_file(EMOTES_TEXT)
        .with_context(|| format!("reading {EMOTES_TEXT}"))?;
    let rs = parse(&bytes, emotes_text_schema(), "EmotesText.dbc")?;
    let mut forms = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let mut cols = [0u32; FORMS];
        for (i, slot) in cols.iter_mut().enumerate() {
            *slot = u32_at(r, 3 + i).unwrap_or(0);
        }
        forms.insert(id, cols);
    }

    let bytes = chain
        .read_file(EMOTES_TEXT_DATA)
        .with_context(|| format!("reading {EMOTES_TEXT_DATA}"))?;
    let rs = parse(&bytes, emotes_text_data_schema(), "EmotesTextData.dbc")?;
    let mut data = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let mut texts: [Option<String>; LOCALES] = Default::default();
        for (locale, slot) in texts.iter_mut().enumerate() {
            // `str_at` already answers `None` for an empty string, which is the reference's
            // "blank for this locale ⇒ take the next rung" test (`cmpb $0,(%edx)`).
            *slot = str_at(&rs, r, 1 + locale);
        }
        data.insert(id, texts);
    }

    Ok(EmoteTextCatalog { forms, data })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WAVE's five reachable enUS forms — checked against the shipped DBC, so a schema slip (a
    /// column-index off-by-one, the wrong locale slot) shows up as the wrong sentence rather than
    /// as a silent empty.
    const WAVE: u32 = 101;

    fn line<'a>(performer: &'a str, target: &'a str, you: &'a str) -> EmoteLine<'a> {
        EmoteLine {
            performer,
            performer_is_you: false,
            performer_female: false,
            target,
            your_name: you,
        }
    }

    #[test]
    fn fill_substitutes_left_to_right_and_keeps_a_surplus_verbatim() {
        assert_eq!(
            fill("%s waves at %s.", &["Bob", "Jane"]),
            "Bob waves at Jane."
        );
        assert_eq!(fill("%s waves at you.", &["Bob"]), "Bob waves at you.");
        assert_eq!(fill("You wave.", &["Bob"]), "You wave.");
        // Surplus verbs stay literal rather than eating the rest of the template.
        assert_eq!(fill("%s waves at %s.", &["Bob"]), "Bob waves at %s.");
        assert_eq!(fill("", &["Bob"]), "");
    }

    /// The real chain: this module is a join between two shipped tables plus a bit-selector, and a
    /// synthetic fixture would only test the plumbing.
    #[test]
    fn the_shipped_tables_compose_waves_five_forms() {
        let data = crate::wow_data_or_skip!();
        let mut chain = Chain::open(&data).expect("open patch chain");
        let cat = load_emote_text_catalog(&mut chain).expect("load");
        assert_eq!(cat.len(), 169, "one form table per EmotesText row");

        // ctx 0 — someone else waves at someone else.
        assert_eq!(
            cat.compose(WAVE, &line("Bob", "Jane", "Me"), 0).as_deref(),
            Some("Bob waves at Jane.")
        );
        // ctx 1 — someone else waves at YOU (the target name matches your own).
        assert_eq!(
            cat.compose(WAVE, &line("Bob", "Me", "Me"), 0).as_deref(),
            Some("Bob waves at you.")
        );
        // ctx 4 — someone else waves at nobody.
        assert_eq!(
            cat.compose(WAVE, &line("Bob", "", "Me"), 0).as_deref(),
            Some("Bob waves.")
        );
        // ctx 2 — YOU wave at someone.
        let mine = EmoteLine {
            performer_is_you: true,
            ..line("Me", "Jane", "Me")
        };
        assert_eq!(
            cat.compose(WAVE, &mine, 0).as_deref(),
            Some("You wave at Jane.")
        );
        // ctx 6 — YOU wave at nobody.
        let mine = EmoteLine {
            performer_is_you: true,
            ..line("Me", "", "Me")
        };
        assert_eq!(cat.compose(WAVE, &mine, 0).as_deref(), Some("You wave."));
    }

    /// **The composer has no self-target case, because it can never be handed one** — and this
    /// test exists to record that, since the shape of the code invites the opposite conclusion
    /// (decision 1282, correcting 1274).
    ///
    /// The branch order is real: `0x49b2c8` matching the performer jumps past the target compare,
    /// so if a self-emote *did* arrive with your own name in the target slot, this is what it
    /// would render. It never arrives. The **send** side zeroes a self-target before the packet
    /// exists (`DoEmote 0x5ef611`), so the server echoes an empty name and the untargeted column
    /// wins — "You wave.", asserted just above. Reading this arm as "what a self-emote does" is
    /// exactly the mistake 1274 made.
    #[test]
    fn the_self_named_target_form_is_unreachable_from_the_wire() {
        let data = crate::wow_data_or_skip!();
        let mut chain = Chain::open(&data).expect("open patch chain");
        let cat = load_emote_text_catalog(&mut chain).expect("load");
        let me = EmoteLine {
            performer_is_you: true,
            ..line("Me", "Me", "Me")
        };
        assert_eq!(
            cat.compose(WAVE, &me, 0).as_deref(),
            Some("You wave at Me.")
        );
        // What the wire actually delivers for that same action — an EMPTY target — and therefore
        // what a player really reads.
        let me = EmoteLine {
            performer_is_you: true,
            ..line("Me", "", "Me")
        };
        assert_eq!(cat.compose(WAVE, &me, 0).as_deref(), Some("You wave."));
    }

    /// The gender rung: WAVE has no female column, so `8|ctx` misses and rung 2 (`ctx` alone)
    /// answers — a female performer must read exactly the same sentence, not nothing.
    #[test]
    fn a_missing_female_column_falls_back_to_the_ungendered_one() {
        let data = crate::wow_data_or_skip!();
        let mut chain = Chain::open(&data).expect("open patch chain");
        let cat = load_emote_text_catalog(&mut chain).expect("load");
        let her = EmoteLine {
            performer_female: true,
            ..line("Ann", "Jane", "Me")
        };
        assert_eq!(
            cat.compose(WAVE, &her, 0).as_deref(),
            Some("Ann waves at Jane.")
        );
    }

    /// A locale column this install does not ship is empty everywhere, so the ladder runs dry and
    /// the composer emits no line — the reference's `0x49b4bd` tail, not a panic and not a blank.
    #[test]
    fn an_unshipped_locale_composes_nothing() {
        let data = crate::wow_data_or_skip!();
        let mut chain = Chain::open(&data).expect("open patch chain");
        let cat = load_emote_text_catalog(&mut chain).expect("load");
        assert_eq!(cat.compose(WAVE, &line("Bob", "Jane", "Me"), 5), None);
        // …and so does an id with no row at all.
        assert_eq!(cat.compose(9999, &line("Bob", "Jane", "Me"), 0), None);
    }

    /// The whole table, not one row. This is the check that would have caught B156's real shape —
    /// "the sentence table is never consulted" reads identically to "this one emote has no text"
    /// until you sweep all 169.
    ///
    /// **Exactly three rows are legitimately silent**, and naming them is the point: `SIT`(86)
    /// points columns 4/6 at `EmotesTextData` rows 446/447, which exist but ship **blank in every
    /// locale**, while `STAND`(141) and `TRAIN`(264) have all sixteen columns zero. The ladder
    /// therefore runs dry for them and the reference prints nothing — which is what a vanilla
    /// `/sit` does. Every other row must compose a `%s`-free sentence in all five reachable
    /// contexts.
    #[test]
    fn every_shipped_emote_composes_except_the_three_silent_rows() {
        let data = crate::wow_data_or_skip!();
        let mut chain = Chain::open(&data).expect("open patch chain");
        let cat = load_emote_text_catalog(&mut chain).expect("load");
        let mut ids: Vec<u32> = cat.forms.keys().copied().collect();
        ids.sort_unstable();
        let mut silent = Vec::new();
        for id in ids {
            for (label, l) in [
                ("other→other", line("Bob", "Jane", "Me")),
                ("other→you", line("Bob", "Me", "Me")),
                ("other→none", line("Bob", "", "Me")),
                (
                    "you→other",
                    EmoteLine {
                        performer_is_you: true,
                        ..line("Me", "Jane", "Me")
                    },
                ),
                (
                    "you→none",
                    EmoteLine {
                        performer_is_you: true,
                        ..line("Me", "", "Me")
                    },
                ),
            ] {
                match cat.compose(id, &l, 0) {
                    None => silent.push(id),
                    Some(s) => {
                        assert!(!s.is_empty(), "EmotesText {id} composed empty for {label}");
                        assert!(
                            !s.contains("%s"),
                            "EmotesText {id} left a %s unfilled for {label}: {s:?}"
                        );
                    }
                }
            }
        }
        silent.dedup();
        assert_eq!(silent, [86, 141, 264], "the silent set is SIT/STAND/TRAIN");
    }
}
