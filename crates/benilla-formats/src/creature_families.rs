//! `CreatureFamily.dbc` + `ItemPetFood.dbc` — the pet's **family word** and the **diet** it implies:
//! `UnitCreatureFamily`'s "Imp"/"Wolf"/"Cat", and `GetPetFoodTypes`'s
//! "Meat, Fish".
//!
//! Both layouts below were **dumped from this install's own shipped files** rather than taken from
//! memory or a wiki — the WDBC header first, then every column of every row, then a census of what
//! each column actually holds. What that dump says:
//!
//! **`CreatureFamily.dbc` — 18 fields, `record_size` 0x48, 23 rows, string block 933 bytes.**
//!
//! | field | byte | holds |
//! |---|---|---|
//! | 0 | 0x00 | `ID` — 1..28 with gaps (10, 13, 14, 18, 22 are absent) |
//! | 1 | 0x04 | `minScale`, a **float** (0.4 … 1.0) |
//! | 2 | 0x08 | `minScaleLevel` — **1 on all 22 real families**, warlock minions included; 0 only on row 28 |
//! | 3 | 0x0c | `maxScale`, a **float** (0.6 … 1.1) |
//! | 4 | 0x10 | `maxScaleLevel` — **60 on all 22**; 0 only on row 28 |
//! | 5 | 0x14 | `skillLine[0]` — a `SkillLine.dbc` id (188…758), distinct on every row |
//! | 6 | 0x18 | `skillLine[1]` — 270 on every tameable family, 0 on the rest |
//! | 7 | 0x1c | **`petFoodMask`** — the bitfield this module's other half indexes |
//! | 8..15 | 0x20 | the localized `Name` block; **enUS is field 8**, the other seven are all zero |
//! | 16 | 0x40 | the name block's locale-present flags (0x7EFFFE or 0x3F007E) — **not** zero |
//! | 17 | 0x44 | `iconFile` — `Interface\Icons\Ability_Hunter_Pet_*` (a string, not a dword) |
//!
//! (The 2026-08-06 dump's note that the level columns are "0 for the warlock ones" was wrong, and
//! is corrected above off the shipped file: Imp/Voidwalker/Succubus/Felhunter/Doomguard all read
//! 1 → 60 like every hunter family, and simply carry `minScale == maxScale` so their ramp is flat.
//! Only "Remote Control" (row 28) is 0/0/0/0. It matters because those four columns are the
//! character-select pet's SIZE — decision 1538.)
//!
//! That confirms the `0x48` record and the 23 rows [`crate::pet_stats`]'s header already recorded,
//! and **corrects its "every dword from `0x28` up is zero"**: fields 10..15 (`0x28`–`0x3c`) are
//! indeed all zero — which is what that argument actually rested on, since the reads it was ruling
//! out were at `+0x28`/`+0x30`/`+0x3c`/`+0x48` — but `0x40` and `0x44` are not. The conclusion
//! stands (a `rec+0x48` read still runs off the end of a 0x48-byte record); the sentence was one
//! column too wide.
//!
//! **`ItemPetFood.dbc` — 10 fields, `record_size` 0x28, 8 rows, string block 55 bytes**: `ID`@0,
//! the localized `Name` block@1..8 (**enUS is field 1**, the rest zero), name flags@9. The eight
//! rows in id order are exactly `Meat / Fish / Cheese / Bread / Fungus / Fruit / Raw Meat /
//! Raw Fish`.
//!
//! **The mask's bit → row mapping is `bit b` → `ItemPetFood` row `b + 1`** (see
//! [`PetFoodNames::for_mask`]) — DATA-derived twice over. The highest bit any shipped row sets is
//! **7** (Turtle, mask 178) and the table has exactly 8 rows, so a 0-based mapping would leave row
//! 8 permanently unreachable and Wolf (mask 1) with an empty diet. And vmangos, reading the same
//! file, tests exactly `1 << (item->FoodType - 1)` against this column (`Objects/Pet.cpp:1503-1504`,
//! `Database/DBCStructure.h:251` — its `CreatureFamilyfmt` `"nfifiiiissssssssxx"` is this table's
//! layout above, field for field). Every resulting diet matches vanilla's documented ones: Wolf =
//! Meat, Cat = Meat+Fish, Bear/Boar = all six, Gorilla = Fungus+Fruit, Wind Serpent = Fish+Cheese+
//! Bread.
//!
//! **All of the above holds in the binary too** (2026-08-06 — `GetPetFoodTypes 0x4bea10`
//! and `UnitCreatureFamily 0x51a310`), and the two dumps agree line for line with nothing left
//! over: the mask is column 7, the bit map is `1 << (recordID - 1)`, the family name is column
//! `8 + locale`, the food name is column `1 + locale`, the id space really does have null rows at
//! 10/13/14/18/22, and the binary's own positive controls (Wolf → Meat; Bear and Boar → all six;
//! Turtle → Fish/Fungus/Fruit/Raw Fish) are the rows asserted below. The one claim that had been
//! flagged INFERRED here — that the walk is **low bit first** — is settled the same way: the client
//! pushes in **record order**, which is what ascending bits produce.
//!
//! What this module deliberately does **not** model is `GetPetFoodTypes`' own `0x6116e0` gate
//! (owner-is-me + the local player is a Hunter). That is a live-state question, not a table one, so
//! it lives at the feed (`benilla_app::ui_pet_stats`) — and it is why a charmed boar under a priest
//! answers an empty diet despite a mask of 63.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::chain::Chain;
use crate::dbc::{f32_at, parse, str_at, u32_at};

const CREATURE_FAMILY: &str = "DBFilesClient\\CreatureFamily.dbc";
const ITEM_PET_FOOD: &str = "DBFilesClient\\ItemPetFood.dbc";

/// `CreatureFamily.dbc`'s column count (the DBC header's `field_count`; `benilla-dbc` enforces it,
/// so a wrong number fails loudly instead of misaligning every column).
const FAMILY_FIELDS: usize = 18;
/// `ItemPetFood.dbc`'s column count.
const FOOD_FIELDS: usize = 10;

/// `CreatureFamily.dbc`'s level → size ramp — bytes `0x04`/`0x08`/`0x0c`/`0x10`, i.e. fields 1–4.
/// (Written as indices, not the `byte / 4` the columns below use: `0x04 / 4` is `4 / 4`, which
/// clippy's `eq_op` reads as a mistake — and it is not worth an allow to keep one idiom.)
const FAMILY_MIN_SCALE_FIELD: usize = 1;
const FAMILY_MIN_SCALE_LEVEL_FIELD: usize = 2;
const FAMILY_MAX_SCALE_FIELD: usize = 3;
const FAMILY_MAX_SCALE_LEVEL_FIELD: usize = 4;
/// `CreatureFamily.dbc`'s pet-food mask — byte `0x1c` = field 7.
const FAMILY_FOOD_MASK_FIELD: usize = 0x1c / 4;
/// `CreatureFamily.dbc`'s enUS `Name` — byte `0x20` = field 8.
const FAMILY_NAME_FIELD: usize = 0x20 / 4;
/// `CreatureFamily.dbc`'s `iconFile` — byte `0x44` = field 17. The last column, past the name
/// block's locale flags; not read by anything yet, typed as a string so the schema is honest.
const FAMILY_ICON_FIELD: usize = 0x44 / 4;
/// `ItemPetFood.dbc`'s enUS `Name` — field 1, the same shape every other localized table here uses.
const FOOD_NAME_FIELD: usize = 1;

/// The number of `ItemPetFood` rows a mask can name. Not a guess at the file's size: it is the
/// width the *mask* can address, and the shipped table happens to fill it exactly (bit 7 → row 8,
/// Turtle's "Raw Fish"). Bits above this are ignored rather than fabricating rows.
const MAX_FOOD_BITS: u32 = 8;

/// One `CreatureFamily.dbc` row, reduced to its live columns: the family word, the diet mask, and
/// the **level → size ramp**.
#[derive(Debug, Clone, PartialEq)]
pub struct CreatureFamily {
    /// The localized family word — "Wolf", "Imp", "Wind Serpent". What `UnitCreatureFamily` pushes.
    pub name: String,
    /// The family's icon path — `Interface\\Icons\\Ability_Hunter_Pet_<Family>` for every one of
    /// the 22 real families (verified by dumping the shipped table this session; the column is
    /// field 17, `0x44`). This is a pet's icon everywhere one is shown without an item behind it:
    /// the stable window's slot buttons and `GetPetIcon`. The schema already
    /// declared the column so the parser aligned past it; only the read into this struct is new.
    ///
    /// Empty for a row that ships no icon — the same "render the empty-slot art" case as a pet
    /// whose creature query has not landed, which is why consumers treat it as absent rather than
    /// passing an empty path to a texture setter (a path resolving to nothing draws
    /// WHITE).
    pub icon: String,
    /// The pet-food bitfield (field 7). **`0` for every warlock minion** — Imp, Voidwalker,
    /// Succubus, Felhunter, Doomguard all ship a zero mask, and so does row 28 ("Remote Control") —
    /// which is why a diet list can be legitimately empty without anything having gone wrong.
    pub pet_food_mask: u32,
    /// The size ramp's ends (fields 1–4): the model scale at `min_scale_level` and at
    /// `max_scale_level`. See [`CreatureFamily::scale_at`].
    pub min_scale: f32,
    pub min_scale_level: u32,
    pub max_scale: f32,
    pub max_scale_level: u32,
}

impl CreatureFamily {
    /// **A pet's render scale at a level** — the character-select pet's whole size law (decision
    /// 1538):
    ///
    /// ```text
    /// S = minScale + (maxScale − minScale) · clamp(level − minScaleLevel, 0, range) / range
    /// range = maxScaleLevel − minScaleLevel
    /// ```
    ///
    /// This is what the reference actually sizes the select pet with. It *also* computes the
    /// familiar `CreatureModelData.modelScale × CreatureDisplayInfo.scale` product one instruction
    /// earlier and then **overwrites it** with this (`0x472e87 fstp`) — the product survives only
    /// when the family lookup misses, which for a real pet it never does.
    ///
    /// Every one of the 22 real families ramps 1 → 60, so a freshly tamed wolf really is visibly
    /// smaller than a level-60 one (Wolf 0.7 → 1.0, Crab 0.7 → 1.4, Spider 0.4 → 0.6); the warlock
    /// minions ship `min == max`, so theirs is flat at every level (Imp 0.5, Voidwalker 0.8,
    /// Succubus and Doomguard 1.0, Felhunter 0.7).
    ///
    /// **`None` for a degenerate ramp** — `range <= 0`, which only row 28 ("Remote Control", all
    /// four columns 0) ships. The reference has no guard there and divides by zero into a NaN
    /// scale; that is a defect of the original, not a mechanism, so we answer "no ramp" and let the
    /// caller take the product fallback the reference itself takes on a lookup miss.
    pub fn scale_at(&self, level: u32) -> Option<f32> {
        let range = self.max_scale_level.checked_sub(self.min_scale_level)?;
        if range == 0 {
            return None;
        }
        let over = level.saturating_sub(self.min_scale_level).min(range);
        let t = over as f32 / range as f32;
        Some(self.min_scale + (self.max_scale - self.min_scale) * t)
    }
}

/// `CreatureFamily.dbc`, by id.
#[derive(Debug, Default, Clone)]
pub struct CreatureFamilies(HashMap<u32, CreatureFamily>);

impl CreatureFamilies {
    /// The row for a family id, or `None`.
    ///
    /// **Family `0` is `None` and that is the common case, not an error**: the file's ids start at
    /// 1, and every creature that is neither a tameable beast nor a warlock minion carries `0` in
    /// the wire's `pet_family` slot. `UnitCreatureFamily`'s nil is this `None`.
    pub fn get(&self, id: u32) -> Option<&CreatureFamily> {
        self.0.get(&id)
    }

    /// The localized family word for an id — [`Self::get`]'s name half.
    /// The family's icon path, or `None` for an unknown family or a row that ships none —
    /// the stable window's slot art and `GetPetIcon`'s answer.
    pub fn icon(&self, id: u32) -> Option<&str> {
        self.0
            .get(&id)
            .map(|f| f.icon.as_str())
            .filter(|i| !i.is_empty())
    }

    pub fn name(&self, id: u32) -> Option<&str> {
        self.get(id).map(|f| f.name.as_str())
    }

    /// A pet's render scale from its family and level — [`CreatureFamily::scale_at`] over
    /// [`Self::get`]. `None` for an unknown family (including the wire's `0`) or a degenerate ramp;
    /// the caller then takes the display's own scale product, as the reference does.
    pub fn pet_scale(&self, family: u32, level: u32) -> Option<f32> {
        self.get(family)?.scale_at(level)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// `ItemPetFood.dbc`: food-type id → its localized name.
#[derive(Debug, Default, Clone)]
pub struct PetFoodNames(HashMap<u32, String>);

impl PetFoodNames {
    /// The name for a food-type id (1-based, as the file numbers them).
    pub fn name(&self, id: u32) -> Option<&str> {
        self.0.get(&id).map(String::as_str)
    }

    /// Expand a [`CreatureFamily::pet_food_mask`] into the localized names it selects — **bit `b`
    /// selects row `b + 1`**, ascending, which is the client's own **record order**
    /// (`0x4bea10`'s `1 << (recordID - 1)`; the module header carries the derivation).
    ///
    /// An empty result is a real answer, not a failure: a warlock minion's family ships mask `0`.
    /// `GetPetFoodTypes` then returns nothing at all, which is exactly what the reference's own
    /// `BuildListString()` turns into nil — and the diet icon that would show it is hidden for a
    /// minion anyway (`HasPetUI`'s second return).
    ///
    /// **This is the table half only.** The binding's live gate — owner-is-me and the local player
    /// is a Hunter (`0x6116e0`) — is the feed's to apply; a charmed boar has a mask of 63 and still
    /// answers nothing under a non-hunter.
    pub fn for_mask(&self, mask: u32) -> Vec<&str> {
        (0..MAX_FOOD_BITS)
            .filter(|b| mask & (1 << b) != 0)
            .filter_map(|b| self.name(b + 1))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn family_schema() -> Schema {
    let mut s = Schema::new("CreatureFamily");
    for i in 0..FAMILY_FIELDS {
        let ty = match i {
            // minScale / maxScale — the only two floats in the record.
            1 | 3 => FieldType::Float32,
            FAMILY_NAME_FIELD | FAMILY_ICON_FIELD => FieldType::String,
            // The rest of the locale block reads as dwords: every one is 0 in the shipped file, and
            // typing them as strings would make the parser chase seven zero offsets per row.
            _ => FieldType::UInt32,
        };
        s.add_field(SchemaField::new(format!("F{i}"), ty));
    }
    s
}

fn food_schema() -> Schema {
    let mut s = Schema::new("ItemPetFood");
    for i in 0..FOOD_FIELDS {
        let ty = if i == FOOD_NAME_FIELD {
            FieldType::String
        } else {
            FieldType::UInt32
        };
        s.add_field(SchemaField::new(format!("F{i}"), ty));
    }
    s
}

/// Load `CreatureFamily.dbc` from the patch chain.
pub fn load_creature_families(chain: &mut Chain) -> Result<CreatureFamilies> {
    let bytes = chain
        .read_file(CREATURE_FAMILY)
        .with_context(|| format!("reading {CREATURE_FAMILY}"))?;
    let rs = parse(&bytes, family_schema(), "CreatureFamily.dbc")?;
    let mut by_id = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        // A row with no enUS name is dropped rather than stored empty: the binding's contract is
        // "a word or nil", and an empty string would render as a trailing space on the level line.
        let Some(name) = str_at(&rs, r, FAMILY_NAME_FIELD).filter(|n| !n.is_empty()) else {
            continue;
        };
        by_id.insert(
            id,
            CreatureFamily {
                name,
                icon: str_at(&rs, r, FAMILY_ICON_FIELD).unwrap_or_default(),
                pet_food_mask: u32_at(r, FAMILY_FOOD_MASK_FIELD).unwrap_or(0),
                min_scale: f32_at(r, FAMILY_MIN_SCALE_FIELD).unwrap_or(1.0),
                min_scale_level: u32_at(r, FAMILY_MIN_SCALE_LEVEL_FIELD).unwrap_or(0),
                max_scale: f32_at(r, FAMILY_MAX_SCALE_FIELD).unwrap_or(1.0),
                max_scale_level: u32_at(r, FAMILY_MAX_SCALE_LEVEL_FIELD).unwrap_or(0),
            },
        );
    }
    Ok(CreatureFamilies(by_id))
}

/// Load `ItemPetFood.dbc` from the patch chain.
pub fn load_pet_food_names(chain: &mut Chain) -> Result<PetFoodNames> {
    let bytes = chain
        .read_file(ITEM_PET_FOOD)
        .with_context(|| format!("reading {ITEM_PET_FOOD}"))?;
    let rs = parse(&bytes, food_schema(), "ItemPetFood.dbc")?;
    let mut by_id = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        if let Some(name) = str_at(&rs, r, FOOD_NAME_FIELD) {
            by_id.insert(id, name);
        }
    }
    Ok(PetFoodNames(by_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain() -> Option<Chain> {
        let data = crate::wow_data_or_skip!(None);
        Some(crate::open_chain(&data).expect("open chain"))
    }

    /// **The level → size ramp, off the shipped file** — the character-select pet's
    /// whole size law, and four columns that fail silently if they slide: misread one and every pet
    /// is simply the wrong size, which no other assertion here would catch.
    ///
    /// The load-bearing shape is that **all 22 real families ramp 1 → 60**, warlock minions
    /// included — they are flat only because `min == max`, not because their level columns are
    /// zero. Row 28 ("Remote Control") is the sole 0/0/0/0 row, and the sole `None`.
    /// The icon column, off the shipped file. It is the stable window's slot art
    /// and `GetPetIcon`'s answer, and it sits at field 17 behind a locale block the schema types as
    /// dwords — so a misread there returns an empty string, and an empty texture path draws WHITE.
    /// Asserting real paths is what catches that.
    #[test]
    fn every_shipped_family_carries_a_pet_icon() {
        let Some(mut chain) = chain() else { return };
        let fams = load_creature_families(&mut chain).expect("families");

        assert_eq!(
            fams.icon(1),
            Some("Interface\\Icons\\Ability_Hunter_Pet_Wolf")
        );
        // A warlock minion has one too — the stable never shows them, but `GetPetIcon` does.
        assert!(fams
            .icon(15)
            .is_some_and(|i| i.starts_with("Interface\\Icons\\")));

        // No real family is missing one, and none is a bare empty string masquerading as a path.
        for id in 1..=27 {
            let Some(f) = fams.get(id) else { continue };
            assert!(
                fams.icon(id).is_some(),
                "family {id} ({}) has no icon",
                f.name
            );
        }
        // An unknown family is absent, not empty.
        assert_eq!(fams.icon(9999), None);
    }

    #[test]
    fn the_family_size_ramp_reads_off_the_shipped_file() {
        let Some(mut chain) = chain() else { return };
        let fams = load_creature_families(&mut chain).expect("families");

        // Every real family ramps across the same 1 → 60 band.
        for (id, f) in (1..=27).filter_map(|id| fams.get(id).map(|f| (id, f))) {
            assert_eq!(
                (f.min_scale_level, f.max_scale_level),
                (1, 60),
                "family {id} ({}) does not ramp 1 → 60",
                f.name
            );
        }

        // A hunter family really does grow: Wolf 0.7 at level 1 → 1.0 at 60, linear between.
        let wolf = fams.get(1).expect("Wolf");
        assert_eq!(wolf.name, "Wolf");
        assert!((wolf.scale_at(1).unwrap() - 0.7).abs() < 1e-6);
        assert!((wolf.scale_at(60).unwrap() - 1.0).abs() < 1e-6);
        let mid = wolf.scale_at(30).unwrap();
        assert!(
            (mid - (0.7 + 0.3 * 29.0 / 59.0)).abs() < 1e-6,
            "midpoint {mid} is not on the line"
        );
        // …and the ramp CLAMPS at both ends rather than extrapolating.
        assert_eq!(wolf.scale_at(0), wolf.scale_at(1));
        assert_eq!(wolf.scale_at(255), wolf.scale_at(60));

        // The warlock minions are flat — same band, `min == max`.
        for (id, want) in [(23, 0.5_f32), (16, 0.8), (15, 0.7), (17, 1.0), (19, 1.0)] {
            let f = fams.get(id).expect("warlock family");
            for level in [1, 30, 60] {
                let s = f.scale_at(level).expect("a real family ramps");
                assert!(
                    (s - want).abs() < 1e-6,
                    "{} at level {level} is {s}, expected a flat {want}",
                    f.name
                );
            }
        }

        // The one degenerate row: no ramp, and NOT a NaN.
        let remote = fams.get(28).expect("Remote Control");
        assert_eq!(remote.scale_at(60), None);

        // Unknown families (the wire's `0` for every non-pet) answer None, not a fabricated size.
        assert_eq!(fams.pet_scale(0, 60), None);
        assert_eq!(fams.pet_scale(10, 60), None); // a real id-space gap
        assert!((fams.pet_scale(1, 60).unwrap() - 1.0).abs() < 1e-6);
    }

    /// The real 5875 `CreatureFamily.dbc`, byte-anchored: 23 rows, the ids with their gaps, and
    /// the two columns we read.
    ///
    /// **The name column is the trap.** Field 8 sits immediately after `petFoodMask` (7) and is
    /// followed by seven zero locale slots — read one column early and every name is a *number*;
    /// read one late and every name is empty. Both fail silently at runtime (a blank level line),
    /// so the names are asserted verbatim, not merely for presence.
    #[test]
    fn the_real_creature_families_name_every_pet() {
        let Some(mut chain) = chain() else { return };
        let f = load_creature_families(&mut chain).expect("load CreatureFamily.dbc");
        assert_eq!(f.len(), 23, "5875 ships 23 creature families");

        // The hunter families, at both ends of the id range.
        assert_eq!(f.name(1), Some("Wolf"));
        assert_eq!(f.name(2), Some("Cat"));
        assert_eq!(f.name(7), Some("Carrion Bird"));
        assert_eq!(f.name(27), Some("Wind Serpent"));
        // …the warlock minions, which is what the pet page's level line reads most often.
        assert_eq!(f.name(15), Some("Felhunter"));
        assert_eq!(f.name(16), Some("Voidwalker"));
        assert_eq!(f.name(17), Some("Succubus"));
        assert_eq!(f.name(19), Some("Doomguard"));
        assert_eq!(f.name(23), Some("Imp"));

        // The id column really has gaps — 10/13/14/18/22 are absent from the shipped file, so a
        // dense 1..=23 assumption would silently shift half the table.
        for missing in [10, 13, 14, 18, 22] {
            assert_eq!(f.name(missing), None, "id {missing} is not in the file");
        }
        // Family 0 is the wire's "no family", and it must be nil rather than the first row.
        assert_eq!(f.name(0), None);
    }

    /// The eight food names, verbatim and in id order — the row set the mask indexes.
    #[test]
    fn the_real_pet_food_names_are_the_eight_diets() {
        let Some(mut chain) = chain() else { return };
        let n = load_pet_food_names(&mut chain).expect("load ItemPetFood.dbc");
        assert_eq!(n.len(), 8);
        for (id, want) in [
            (1, "Meat"),
            (2, "Fish"),
            (3, "Cheese"),
            (4, "Bread"),
            (5, "Fungus"),
            (6, "Fruit"),
            (7, "Raw Meat"),
            (8, "Raw Fish"),
        ] {
            assert_eq!(n.name(id), Some(want), "food id {id}");
        }
        assert_eq!(n.name(0), None, "the file is 1-based; there is no row 0");
        assert_eq!(n.name(9), None);
    }

    /// **The join, on the real data** — the half neither file can check alone, and the one that
    /// would fail silently as "a bear eats fish and cheese".
    ///
    /// Wolf, Bear, Boar and Turtle are **the binary's own positive controls** (`0x4bea10`,
    /// 2026-08-06); the rest are vanilla's documented diets, which is the
    /// independent corroboration that `bit b → row b+1` is right (vmangos's
    /// `1 << (FoodType - 1)` is the third).
    #[test]
    fn the_shipped_masks_expand_to_vanillas_own_diets() {
        let Some(mut chain) = chain() else { return };
        let fam = load_creature_families(&mut chain).expect("families");
        let food = load_pet_food_names(&mut chain).expect("foods");
        let diet = |id: u32| food.for_mask(fam.get(id).expect("family").pet_food_mask);

        const EVERYTHING: [&str; 6] = ["Meat", "Fish", "Cheese", "Bread", "Fungus", "Fruit"];
        assert_eq!(diet(1), ["Meat"], "Wolf — control");
        assert_eq!(diet(2), ["Meat", "Fish"], "Cat");
        assert_eq!(diet(4), EVERYTHING, "Bear — control");
        assert_eq!(diet(5), EVERYTHING, "Boar — control");
        assert_eq!(diet(9), ["Fungus", "Fruit"], "Gorilla");
        assert_eq!(diet(12), ["Cheese", "Fungus", "Fruit"], "Tallstrider");
        assert_eq!(diet(27), ["Fish", "Cheese", "Bread"], "Wind Serpent");
        // Turtle (mask 178) is the control that PINS the bit width: it is the only family to set
        // bit 7, and bit 7 → row 8 is what makes "Raw Fish" reachable at all.
        assert_eq!(
            diet(21),
            ["Fish", "Fungus", "Fruit", "Raw Fish"],
            "Turtle — control"
        );

        // **Exactly which families have an EMPTY diet**, because the pet page's diet tooltip
        // depends on it: `BuildListString()` returns nil for an empty list and `format`'s "%s"
        // errors on nil, so the icon is safe only while no HUNTER-tameable family has a zero mask.
        // The shipped file's zero-mask set is the five warlock minions plus row 28 ("Remote
        // Control") — no tameable beast among them. Asserted as an exact set, not a spot check: a
        // future row with a zero mask is exactly the thing that would break that tooltip.
        let empty: Vec<u32> = {
            let mut v: Vec<u32> = fam
                .0
                .iter()
                .filter(|(_, f)| food.for_mask(f.pet_food_mask).is_empty())
                .map(|(id, _)| *id)
                .collect();
            v.sort_unstable();
            v
        };
        assert_eq!(
            empty,
            [15, 16, 17, 19, 23, 28],
            "Felhunter/Voidwalker/Succubus/Doomguard/Imp + Remote Control — no tameable beast"
        );
    }

    /// The mask expansion's own edges, away from the shipped data: an unknown bit names no row
    /// rather than panicking or inventing one, and bits past the table are ignored.
    #[test]
    fn mask_bits_past_the_table_are_ignored() {
        let mut names = HashMap::new();
        names.insert(1, "Meat".to_string());
        names.insert(3, "Cheese".to_string());
        let n = PetFoodNames(names);

        assert_eq!(n.for_mask(0), Vec::<&str>::new());
        assert_eq!(n.for_mask(0b101), ["Meat", "Cheese"]);
        // Bit 1 names row 2, which this cut-down table doesn't have: skipped, not empty-stringed.
        assert_eq!(n.for_mask(0b111), ["Meat", "Cheese"]);
        // Bit 31 is past `MAX_FOOD_BITS` entirely.
        assert_eq!(n.for_mask(0x8000_0001), ["Meat"]);
    }
}
