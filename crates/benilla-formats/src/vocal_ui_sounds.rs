//! `VocalUISounds.dbc` — the **error speech** lines: the "I can't do that" family your character
//! says aloud in their own race and gender voice when the client refuses something.
//!
//! One row per `(race, line)`; the *line* is the message catalog's own `type_tag`
//! ([`benilla_ui::messages::MessageRecord::type_tag`], the record's `+0x0c`), so a refusal that
//! reaches `CGGameUI::DisplayError` already names its voice line. Its `0x44` value is the "no
//! speech, play the named cue instead" sentinel — which is exactly the table's width, and exactly
//! one past the shipped file's largest `VocalUIEnum` (67).
//!
//! Layout — VERIFIED against build 5875 (`WoW/Data/patch.MPQ`, decoded 2026-09-01): the WDBC header
//! reports **533 records · 7 fields · 28 B/record**, matching the `imul ebx,ebx,0x1c` stride the
//! reference's table builder walks with (`0x45813d`). Fields, read off that builder's own loads:
//!
//! | field | offset | what | where the reference reads it |
//! |---|---|---|---|
//! | 0 | `+0x00` | `ID` | — |
//! | 1 | `+0x04` | [`VocalUiSound::line`] (`VocalUIEnum`), kept only when `< 0x44` | `0x45815c`/`0x45815f` |
//! | 2 | `+0x08` | [`VocalUiSound::race`] | `0x45816b` (equality against the player's race) |
//! | 3 | `+0x0c` | [`VocalUiSound::normal`]`[0]` — male | `0x458173` → `[0xb06240 + line*12]` |
//! | 4 | `+0x10` | [`VocalUiSound::normal`]`[1]` — female | `0x458180` → `[0xb06570 + line*12]` |
//! | 5 | `+0x14` | [`VocalUiSound::pissed`]`[0]` — male | `0x458190` → `[0xb06244 + line*12]` |
//! | 6 | `+0x18` | [`VocalUiSound::pissed`]`[1]` — female | `0x4581a3` → `[0xb06574 + line*12]` |
//!
//! The sex grouping is the load-bearing half of that decode and it is doubly confirmed: the two
//! sex blocks sit `0x330` = `0x44 * 12` bytes apart, and the shipped data's own kit **names** agree
//! row for row — line 0 resolves `HumanMale_InventoryFull` / `HumanFemale_InventoryFull`, and
//! message id 0 (`ERR_INV_FULL`) carries `type_tag` 0.
//!
//! **A sound id here can be `0` or `0xFFFF_FFFF`, and both mean "nothing".** The reference stores
//! the raw field and lets its id-indexed kit lookup (`0x45cda0`: `cmp ecx,count; jae → NULL`) turn
//! both into a miss — `-1` unsigned is past the count, and slot 0 of the table is empty because
//! `SoundEntries` has no id 0. 190 of the file's 1 066 normal ids are `-1` and 8 are `0`; a
//! `(race, sex, line)` with no kit is simply silent. [`VocalUiSound::normal_kit`] is that filter,
//! written once so no consumer re-derives it.

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};

const VOCAL_UI_SOUNDS: &str = "DBFilesClient\\VocalUISounds.dbc";

/// How many error-speech lines the reference's table holds — `0x44`, the bound its builder
/// (`0x45815f cmp eax,0x44; jge`) and its player (`0x45829c cmp edi,0x44; jge`) both test, and the
/// width of each of the two sex blocks. Also the message catalog's "no speech" sentinel: a
/// `type_tag` of exactly `0x44` means *play the record's named cue instead*.
pub const VOCAL_UI_LINES: usize = 0x44;

/// One `VocalUISounds.dbc` row — the kits one race says one line with, per sex.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VocalUiSound {
    pub id: u32,
    /// `VocalUIEnum` — the message catalog's `type_tag`. Always `< VOCAL_UI_LINES` in a row this
    /// loader kept; the reference drops the others at table-build time and so do we.
    pub line: u32,
    /// `ChrRaces` id (the descriptor's `UNIT_FIELD_BYTES_0` race byte).
    pub race: u32,
    /// The line's ordinary delivery, by sex (`0` male, `1` female). `0`/`-1` = none.
    pub normal: [u32; 2],
    /// The **annoyed** delivery, by sex — what the reference escalates to after four consecutive
    /// plays of the same line. Every one of the shipped file's 1 066 ids is absent from
    /// `SoundEntries.dbc`, so this column is **dormant in 5875's data**; it is carried because the
    /// mechanism reads it, not because anything can be heard from it.
    pub pissed: [u32; 2],
}

impl VocalUiSound {
    /// The kit for this row's ordinary line at `sex`, with the reference's own two misses folded
    /// in (`0` and `-1` both resolve to no kit — module doc). `sex` past 1 is `None`: the
    /// reference refuses those at its own front door (`0x458254`/`0x45825b`).
    pub fn normal_kit(&self, sex: u32) -> Option<u32> {
        kit(self.normal, sex)
    }

    /// The annoyed line's kit at `sex` — [`Self::normal_kit`]'s twin (and, on 5875's data, always
    /// a kit that resolves to nothing; see [`Self::pissed`]).
    pub fn pissed_kit(&self, sex: u32) -> Option<u32> {
        kit(self.pissed, sex)
    }
}

fn kit(pair: [u32; 2], sex: u32) -> Option<u32> {
    let id = *pair.get(sex as usize)?;
    (id != 0 && id != u32::MAX).then_some(id)
}

/// The whole table, as rows. Consumers build the reference's per-race lookup from it
/// (`benilla_app::sound::vocal`); this crate stays the file's reader and nothing more.
pub struct VocalUiSoundCatalog {
    rows: Vec<VocalUiSound>,
}

impl VocalUiSoundCatalog {
    /// Every kept row, in **file order** — which matters: the reference builds its table by
    /// walking the file *backwards* (`0x458140`), so on a duplicate `(race, line)` the
    /// lowest-indexed row is the one left standing. A consumer that folds these in order gets the
    /// same answer. (5875's file has no duplicate pair, so this is a rule with no live case.)
    pub fn rows(&self) -> &[VocalUiSound] {
        &self.rows
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// Load `VocalUISounds.dbc` off the patch chain, dropping the rows the reference's builder drops
/// (`VocalUIEnum >= 0x44`).
pub fn load_vocal_ui_sounds(chain: &mut Chain) -> Result<VocalUiSoundCatalog> {
    let bytes = chain
        .read_file(VOCAL_UI_SOUNDS)
        .with_context(|| format!("reading {VOCAL_UI_SOUNDS}"))?;
    let mut schema = Schema::new("VocalUISounds");
    for i in 0..7 {
        schema.add_field(SchemaField::new(format!("f{i}"), FieldType::UInt32));
    }
    let rs = parse(&bytes, schema, "VocalUISounds")?;
    let mut rows = Vec::new();
    for r in rs.records() {
        let (Some(id), Some(line), Some(race)) = (u32_at(r, 0), u32_at(r, 1), u32_at(r, 2)) else {
            continue;
        };
        if line as usize >= VOCAL_UI_LINES {
            continue;
        }
        let f = |i| u32_at(r, i).unwrap_or(0);
        rows.push(VocalUiSound {
            id,
            line,
            race,
            normal: [f(3), f(4)],
            pissed: [f(5), f(6)],
        });
    }
    Ok(VocalUiSoundCatalog { rows })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real 5875 file, and the facts the decode rests on: its shape, its `VocalUIEnum` domain
    /// (dense `0..=67`, one short of the sentinel), and the **sex column order** — read back
    /// through `SoundEntries` names, which is the check that would have caught a transposed pair.
    /// Skips without client data.
    #[test]
    fn real_vocal_ui_sounds_decode_with_the_sexes_the_right_way_round() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_vocal_ui_sounds(&mut chain).expect("load VocalUISounds");
        let kits = crate::load_sound_kit_catalog(&mut chain).expect("load SoundEntries");
        assert_eq!(cat.len(), 533, "every shipped row is under the 0x44 bound");

        let lines: std::collections::BTreeSet<u32> = cat.rows().iter().map(|r| r.line).collect();
        assert_eq!(lines.iter().copied().max(), Some(67));
        assert_eq!(lines.len(), VOCAL_UI_LINES, "the enum is dense 0..=0x43");

        // Human (race 1), line 0 = the message catalog's `ERR_INV_FULL` tag. Male and female must
        // land on their own voices — the whole point of the `+0x0c`/`+0x10` pair.
        let human_inv_full = cat
            .rows()
            .iter()
            .find(|r| r.race == 1 && r.line == 0)
            .expect("Human line 0");
        let name = |kit: Option<u32>| {
            kit.and_then(|k| kits.get(k))
                .map(|k| k.name.clone())
                .unwrap_or_default()
        };
        assert_eq!(
            name(human_inv_full.normal_kit(0)),
            "HumanMale_InventoryFull"
        );
        assert_eq!(
            name(human_inv_full.normal_kit(1)),
            "HumanFemale_InventoryFull"
        );
        assert_eq!(
            human_inv_full.normal_kit(2),
            None,
            "sex past 1 has no voice"
        );

        // The naming law holds across races and lines, so a column slip anywhere shows up here.
        for (race, prefix) in [(3u32, "Dwarf"), (6, "Tauren"), (8, "Troll")] {
            let row = cat
                .rows()
                .iter()
                .find(|r| r.race == race && r.line == 0)
                .expect("row");
            assert_eq!(
                name(row.normal_kit(0)),
                format!("{prefix}Male_InventoryFull")
            );
            assert_eq!(
                name(row.normal_kit(1)),
                format!("{prefix}Female_InventoryFull")
            );
        }
    }

    /// **The annoyed column is dormant in 5875** — every one of its ids is absent from
    /// `SoundEntries.dbc`, so the reference's escalation has nothing to escalate *to* and the
    /// ordinary line plays every time. Asserted rather than assumed, because the play state
    /// machine's whole observable behaviour turns on it (`benilla_app::sound::vocal`). Skips
    /// without client data.
    #[test]
    fn the_annoyed_column_resolves_to_nothing_in_this_build() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_vocal_ui_sounds(&mut chain).expect("load VocalUISounds");
        let kits = crate::load_sound_kit_catalog(&mut chain).expect("load SoundEntries");
        let live = cat
            .rows()
            .iter()
            .flat_map(|r| [r.pissed_kit(0), r.pissed_kit(1)])
            .flatten()
            .filter(|k| kits.get(*k).is_some())
            .count();
        assert_eq!(
            live, 0,
            "an annoyed line resolved — 1815's dormancy is over"
        );
        // The control: the ordinary column is mostly live, so the sweep above is testing the data
        // and not a broken lookup.
        let normal_live = cat
            .rows()
            .iter()
            .flat_map(|r| [r.normal_kit(0), r.normal_kit(1)])
            .flatten()
            .filter(|k| kits.get(*k).is_some())
            .count();
        assert_eq!(normal_live, 785);
    }
}
