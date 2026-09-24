//! `ChrClasses.dbc` — the per-class client table, and the three columns anything here reads off
//! it.
//!
//! One file, one parse. Two of the three are answers the *engine* gives Lua about a class and the
//! third is the talent spell-modifier gate's right-hand side; all three are read by address off
//! the same class-indexed record table at `ds:0xc0def4`, bounded by the max id at `ds:0xc0def8`
//! (store `0xc0deec`, loader `0x542360`, filename `0x85838c`).
//!
//! ## Field 4 — the pet name token
//!
//! `HasPetSpells()`'s second return, and the only thing that decides whether the spellbook's
//! second tab reads "Pet" or "Demon" (decision 1032). `HasPetSpells 0x4b4410` pushes it verbatim:
//!
//! ```text
//! 0x4b445d  eax = [player + 0x110]                  ; the descriptor fields
//! 0x4b4463  eax = byte [fields + 0x79]              ; UNIT_FIELD_BYTES_0 byte 1 = the CLASS
//! 0x4b446b  if (class < 0 || class > [0xc0def8]) -> the out-of-range arm
//! 0x4b4473  ecx = [0xc0def4]                        ; the class record table
//! 0x4b4479  eax = ecx[class]                        ; 1-BASED — index 0 is never a class
//! 0x4b447c  edx = [rec + 0x10]                      ; <- the token string
//! 0x4b44a6  (player did not resolve) edx = "PET"    ; the literal 0x846a40
//! ```
//!
//! **`rec + 0x10` is field 4, and the file says which field that is**: dumped from the real 5875
//! `ChrClasses.dbc` (9 rows, 17 columns, 0x44-byte records), field 4 is `"PET"` in every row except
//! Warlock (id 9), which is `"DEMON"`. Field 5 is the class name (`Warrior`/`Paladin`/…), so a
//! one-column slip would put "Warlock" on the tab — plausible enough to ship, which is why
//! [`tests`] anchors both columns.
//!
//! FrameXML then does `getglobal("PET_TYPE_"..token)` (`SpellBookFrame.lua:173`) against
//! `PET_TYPE_PET = "Pet"` / `PET_TYPE_DEMON = "Demon"` (`GlobalStrings.lua:3057-3058`) — so the
//! token is a **key**, never display text, and it is right that it is not localized here.
//!
//! ## Field 16 — the relic-slot flag
//!
//! `UnitHasRelicSlot 0x519e50` reads it and nothing else. Same walk — typemask bit 4 (PLAYER),
//! `UNIT_FIELD_BYTES_0` byte 1 for the class, bound against `0xc0def8`, index `0xc0def4` — then:
//!
//! ```text
//! 0x519ebb  mov ecx,[row + 0x40]                    ; <- field 16, the relic flag
//! ```
//!
//! Non-zero pushes the **number 1.0**; zero pushes **nil** (never `false`). There is no `cmp`
//! against a class id anywhere in the function: 1.12 is entirely data-driven here, and this table
//! is the data. In the shipped file the column is 1 for exactly **2 Paladin, 7 Shaman, 11 Druid**
//! — Libram, Totem, Idol.
//!
//! **Why this was believed impossible, and the trap worth keeping.** The base `dbc.MPQ` copy of
//! this file is **16 fields / 64-byte records and has no field 16 at all**; `patch.MPQ` supersedes
//! it with the 17-field / 68-byte version, and the 5875 loader asserts exactly those two numbers
//! (`0x54240e`/`0x542446`), so the patch copy is the only one the client can read. Read the base
//! archive alone — or an early-vanilla 16-column struct — and the flag simply is not there, which
//! is how "the relic slot post-dates 1.12" became a settled belief in this codebase and stayed one
//! across two decision records. It is false; see decision 1796. [`CHR_CLASSES_FIELDS`] is what
//! keeps us on the patch copy, and it is load-bearing, not defensive.
//!
//! The flag is engine-enforced, not a UI conceit: `IsValidForSlot 0x5da1d0`'s `slot == 0x11` leg
//! requires `(InventoryType == 28 RELIC) == hasRelicSlot`, so **INVSLOT 17 takes a relic for those
//! three classes and a ranged weapon for everyone else** — one slot, two meanings. 1.12 has no
//! *separate* relic slot, which is the one true fragment inside the old belief.
//!
//! ## Field 15 — the class's spell family
//!
//! `GetSpellModifiers 0x6e6b30`'s second gate: a talent modifier applies to a spell only when that
//! spell's own `SpellFamilyName` (`Spell.dbc` column 160) equals the LOCAL PLAYER's class family,
//! which the client caches in the global `[0xcecaac]`. That global has exactly one
//! non-zeroing writer, `0x6e6ca0` (sole caller `0x5debcc`, the local-player create-finalise): it
//! takes `UNIT_FIELD_BYTES_0` byte 1 — the class, the same walk both sections above make — and
//! stores `ChrClasses[class] + 0x3c`, i.e. **field 15**.
//!
//! Anchored on the shipped 5875 file, which is also a could-have-failed control on the column: the
//! nine rows read Warrior 4, Paladin 10, Hunter 9, Rogue 8, Priest 6, Shaman 11, Mage 3, Warlock 5,
//! Druid 7 — the vmangos `SpellFamilyNames` values exactly, in an order no neighbouring column
//! could reproduce (field 14 is a name-flags word reading 8323199 on every row, field 16 is the
//! relic flag above).

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::chain::Chain;
use crate::dbc::{parse, str_at, u32_at};

const CHR_CLASSES: &str = "DBFilesClient\\ChrClasses.dbc";

/// `ChrClasses.dbc`'s column count in 5875 (the header's `field_count`; `benilla-dbc` enforces it).
///
/// This is the **patch** copy's shape. The base archive's 16-field copy fails the check rather
/// than silently reading one column short of the relic flag — see the module header.
const CHR_CLASSES_FIELDS: usize = 17;

/// The pet-name-token column — byte `0x10` in the client's own record read, i.e. field 4.
const PET_NAME_TOKEN_FIELD: usize = 0x10 / 4;

/// The relic-slot column — byte `0x40` in `UnitHasRelicSlot`'s read, i.e. field 16.
const RELIC_SLOT_FIELD: usize = 0x40 / 4;

/// The class spell-family column — byte `0x3c` in `0x6e6ca0`'s read, i.e. field 15. See the module
/// header for the walk and the shipped-file anchor.
const SPELL_FAMILY_FIELD: usize = 0x3c / 4;

/// The literal the client pushes when the player object does not resolve (`0x846a40`). Also the
/// value nine of the ten class rows carry, which is why an unloaded table degrades invisibly.
pub const PET_NAME_TOKEN_FALLBACK: &str = "PET";

/// One class row, narrowed to the columns anything reads.
#[derive(Debug, Clone)]
struct ChrClass {
    pet_name_token: Option<String>,
    has_relic_slot: bool,
    spell_family: u32,
}

/// `ChrClasses.dbc` → class id ⇒ the columns we read.
#[derive(Debug, Default, Clone)]
pub struct ChrClasses(HashMap<u32, ChrClass>);

impl ChrClasses {
    /// The pet name token for a class id, or the client's own [`PET_NAME_TOKEN_FALLBACK`] for a
    /// class with no row. Mirrors `0x4b44a6`: the reference never answers nil here, only ever a
    /// string.
    pub fn pet_name_token(&self, class: u32) -> &str {
        self.0
            .get(&class)
            .and_then(|c| c.pet_name_token.as_deref())
            .unwrap_or(PET_NAME_TOKEN_FALLBACK)
    }

    /// Whether a class id's INVSLOT 17 is a relic slot — `UnitHasRelicSlot`'s whole body.
    ///
    /// A class with no row answers `false`, which is the reference's own shape: its bound check
    /// against `0xc0def8` falls to the nil leg, and an unloaded table reads every class as an
    /// ordinary ranged wielder rather than inventing a relic slot for one.
    pub fn has_relic_slot(&self, class: u32) -> bool {
        self.0.get(&class).is_some_and(|c| c.has_relic_slot)
    }

    /// The class's spell family — the value the reference caches in `[0xcecaac]` and compares
    /// every spell's `SpellFamilyName` against before any talent modifier may apply.
    ///
    /// A class with no row answers `0`, which is the reference's own degraded state: `0x6e7150`
    /// zeroes the global at world-enter (`6e7317`, `esi = 0`) and it stays 0 until the local
    /// player resolves — and `GetSpellModifiers`' first conjunct (`SpellFamilyName != 0`) exists
    /// precisely so nothing matches during that window.
    pub fn spell_family(&self, class: u32) -> u32 {
        self.0.get(&class).map_or(0, |c| c.spell_family)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("ChrClasses");
    for i in 0..CHR_CLASSES_FIELDS {
        // Only the two read columns are typed; everything else stays an opaque dword. The class
        // NAME block (field 5 up) is deliberately not decoded — nothing here displays it.
        let ty = if i == PET_NAME_TOKEN_FIELD {
            FieldType::String
        } else {
            FieldType::UInt32
        };
        s.add_field(SchemaField::new(format!("F{i}"), ty));
    }
    s
}

/// Load `ChrClasses.dbc`'s read columns from the patch chain.
pub fn load_chr_classes(chain: &mut Chain) -> Result<ChrClasses> {
    let bytes = chain
        .read_file(CHR_CLASSES)
        .with_context(|| format!("reading {CHR_CLASSES}"))?;
    let rs = parse(&bytes, schema(), "ChrClasses.dbc")?;
    let mut by_id = HashMap::new();
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        by_id.insert(
            id,
            ChrClass {
                pet_name_token: str_at(&rs, r, PET_NAME_TOKEN_FIELD),
                has_relic_slot: u32_at(r, RELIC_SLOT_FIELD).is_some_and(|v| v != 0),
                spell_family: u32_at(r, SPELL_FAMILY_FIELD).unwrap_or(0),
            },
        );
    }
    Ok(ChrClasses(by_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain() -> Option<crate::chain::Chain> {
        let data = crate::wow_data_or_skip!(None);
        Some(crate::open_chain(&data).expect("open chain"))
    }

    /// The real 5875 table, byte-anchored on the one column that matters. The neighbouring field
    /// holds the class NAME, so a one-column slip reads "Warlock"/"Hunter" — which would still
    /// resolve a `PET_TYPE_*` global lookup to nil and blank the tab, i.e. fail silently.
    #[test]
    fn warlocks_pet_is_a_demon_and_everyone_elses_is_a_pet() {
        let Some(mut chain) = chain() else { return };
        let t = load_chr_classes(&mut chain).expect("load ChrClasses.dbc");
        assert!(!t.is_empty());
        assert_eq!(t.pet_name_token(9), "DEMON", "Warlock");
        for (class, who) in [(1, "Warrior"), (3, "Hunter"), (11, "Druid")] {
            assert_eq!(t.pet_name_token(class), "PET", "{who}");
        }
        // 6 and 10 have no row in 1.12 (Death Knight / Monk arrive later); the client's own
        // out-of-range arm answers the literal, and so does this.
        assert_eq!(t.pet_name_token(6), PET_NAME_TOKEN_FALLBACK);
        assert_eq!(t.pet_name_token(0), PET_NAME_TOKEN_FALLBACK);
    }

    /// Field 16, anchored against the shipped file the same way — and against the belief it
    /// replaces. Three classes, and exactly three: this asserts the whole column, because a
    /// half-right answer here (say, Paladin alone) is the shape the old claim would decay into.
    #[test]
    fn libram_totem_and_idol_are_the_three_relic_classes() {
        let Some(mut chain) = chain() else { return };
        let t = load_chr_classes(&mut chain).expect("load ChrClasses.dbc");
        for (class, who) in [(2, "Paladin"), (7, "Shaman"), (11, "Druid")] {
            assert!(t.has_relic_slot(class), "{who} carries a relic");
        }
        for (class, who) in [
            (1, "Warrior"),
            (3, "Hunter"),
            (4, "Rogue"),
            (5, "Priest"),
            (8, "Mage"),
            (9, "Warlock"),
        ] {
            assert!(!t.has_relic_slot(class), "{who} wields a ranged weapon");
        }
        // No row, and no invented slot — the reference's nil leg.
        assert!(!t.has_relic_slot(6));
        assert!(!t.has_relic_slot(0));
    }

    /// Field 15, the spell-family gate's right-hand side. The whole column is asserted because
    /// that is what makes it a column pin rather than a coincidence: nine distinct values in nine
    /// rows, each matching the vmangos `SpellFamilyNames` enum, is a shape no neighbour has
    /// (field 14 is 8323199 on every row, field 16 is the three-row relic flag).
    #[test]
    fn every_class_row_carries_its_vmangos_spell_family() {
        let Some(mut chain) = chain() else { return };
        let t = load_chr_classes(&mut chain).expect("load ChrClasses.dbc");
        for (class, family, who) in [
            (1u32, 4u32, "Warrior"),
            (2, 10, "Paladin"),
            (3, 9, "Hunter"),
            (4, 8, "Rogue"),
            (5, 6, "Priest"),
            (7, 11, "Shaman"),
            (8, 3, "Mage"),
            (9, 5, "Warlock"),
            (11, 7, "Druid"),
        ] {
            assert_eq!(t.spell_family(class), family, "{who}");
        }
        // No row: the reference's zeroed global, which conjunct 1 of the gate refuses.
        assert_eq!(t.spell_family(6), 0);
        assert_eq!(t.spell_family(0), 0);
    }
}
