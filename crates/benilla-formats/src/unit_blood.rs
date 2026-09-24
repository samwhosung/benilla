//! **UnitBlood.dbc / UnitBloodLevels.dbc** — the melee blood-spurt tables (decision 0137 phase 3).
//!
//! The chain (`0x624530 → 0x625010`): a melee hit with
//! `HitInfo & 0x2`, nonzero damage, and victimState ∈ {1,4} resolves the victim's blood id
//! ([`BloodCatalog::level_key`]: the `CreatureDisplayInfo.BloodLevel` override, else
//! `CreatureModelData.BloodID`, else **UnitBloodLevels' file row 0** — the tier-3 fallback 1850
//! restored) → a **UnitBloodLevels** row, whose three columns are the bloodID per
//! `violenceLevel` (0–2) — the
//! vanilla censorship table: a red-blooded creature at violence 1 bleeds *green*, at 0 nothing —
//! → a **UnitBlood** row whose first four fields are `SpellVisualEffectName` ids. Column order
//! pinned empirically from the effect names (`Combat Blood Spurt <Front|Back> <Small|Large> <color>`,
//! 12/12 rows consistent): **[FrontSmall, FrontLarge, BackSmall, BackLarge]** — front/back from
//! `sign(victimForward · (attackerPos − victimPos))`, Large on the *crushing* bit `HitInfo & 0x2000`
//! (not crit — crit `0x80` belongs to the wound flinch).
//!
//! **Fields 5–9 are the row's ground-splat decal textures, and they are DEAD in 1.12.1**
//! (`textures\BloodSplats\…`, [`BloodCatalog::splats`]).
//! Not "unimplemented" — *absent*: no instruction in the image reads
//! `UnitBloodRecord + 0x14 … +0x24`, and the string `"BloodSplat"` occurs nowhere in `WoW.exe`,
//! though all twelve `.blp` ship in `texture.MPQ` and the DBC resolves them. Data shipped, art
//! shipped, code gone. The only ground decal in 1.12.1 that takes a DBC string column is the
//! footprint. So there is no trigger, no placement and no fade to be faithful to, and the accessor
//! exists to pin the finding, not to feed a lane: the shipped table names **four** textures per
//! row and leaves the fifth column empty in all three (red / green / black), so a reader takes the
//! non-empty prefix, never a fixed five. Drawing them at all would be a benilla embellishment —
//! the director's call, not a fidelity gap.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};

const UNIT_BLOOD: &str = "DBFilesClient\\UnitBlood.dbc";
const UNIT_BLOOD_LEVELS: &str = "DBFilesClient\\UnitBloodLevels.dbc";

/// The two blood tables, resolving `(blood id, violence level, front, crushing)` → the spurt's
/// `SpellVisualEffectName` id (whose path the spell-visual catalog resolves like any kit effect).
pub struct BloodCatalog {
    /// UnitBloodLevels: id → the bloodID per violence level 0/1/2 (`0` = no blood at that level).
    levels: HashMap<u32, [u32; 3]>,
    /// The id of UnitBloodLevels' **file row 0** — the records base the reference's tier-3
    /// fallback returns ([`BloodCatalog::level_key`]). `1` in the shipped table (ids 1/2/3, so
    /// this is emphatically *not* `idIndex[0]`, which is NULL). `None` for an empty table.
    default_level: Option<u32>,
    /// UnitBlood: id → the row's spurt effects + ground-splat textures.
    rows: HashMap<u32, BloodRow>,
}

/// One `UnitBlood.dbc` row: the four spurt `SpellVisualEffectName` ids and the row's ground-splat
/// decal textures.
struct BloodRow {
    /// `[FrontSmall, FrontLarge, BackSmall, BackLarge]` effect ids.
    effects: [u32; 4],
    /// The non-empty ground-splat textures, in column order (`textures\BloodSplats\…`,
    /// extensionless). Four in every shipped row; the fifth column is empty throughout.
    splats: Vec<String>,
}

impl BloodCatalog {
    /// The victim's **UnitBloodLevels key**, by the reference's three-tier fallback — the display
    /// resolve `0x60afb0`, which stores the resulting row pointer at `[unit+0xb48]` and is the only
    /// place blood ids are turned into a row:
    ///
    /// 1. `CreatureDisplayInfo.BloodLevel` (`display_blood`), else
    /// 2. `CreatureModelData.BloodID` (`model_blood`), else
    /// 3. **UnitBloodLevels' file row 0** — id `1`, the RED row, in the shipped table.
    ///
    /// A tier "fails" on `< 0`, on `> maxId`, **or** on an `idIndex` slot that holds no row —
    /// which a [`HashMap`] miss covers exactly. That third tier is the load-bearing one: 595 of
    /// the 10534 shipped displays (5.6 %) reach it, because `BloodID = −1` is a tier-2 *miss*,
    /// not a bloodless marker. Reading `−1` as "bloodless" is what benilla did until 1850, and
    /// it silently dropped the spurt on every one of those 595.
    ///
    /// **The content proves tier 3 on its own** (1859), which matters because reading a
    /// `setle/dec/and` correctly is a thing one can be wrong about, and this is not. The tier-3
    /// population is headed by Quilboar, Mountain Giants, Crocolisks, Gnolls and Naga Sirens —
    /// `Crocodile.mdx` being the *only* crocolisk model, so "no blood" here means no crocolisk in
    /// the game ever bleeds. `−1` is **unspecified**, not *none*, and it splits families that
    /// share a directory: `QuillBoar` is `−1` beside `QuillBoarWarrior` at `1`, `GnollMelee`
    /// beside `GnollCaster`, `Troll\TrollMelee` beside `Troll\Troll`. It even splits a single
    /// creature — eight models ship the same art under two `CreatureModelData` rows whose blood
    /// ids disagree with one side `−1` (a Baby Murloc is both `1` and `−1`).
    ///
    /// `None` only for an empty table — no shipped display is bloodless by this chain. A unit
    /// still ends up with no blood at violence 0, where every row's column is `0`.
    pub fn level_key(&self, display_blood: i32, model_blood: i32) -> Option<u32> {
        let resolves = |v: i32| {
            u32::try_from(v)
                .ok()
                .filter(|k| self.levels.contains_key(k))
        };
        resolves(display_blood)
            .or_else(|| resolves(model_blood))
            .or(self.default_level)
    }

    /// The spurt effect for a victim: `blood_id` is the victim's UnitBloodLevels key — what
    /// [`Self::level_key`] resolved, which for shipped data is always a real row — `violence` the
    /// gore level (0–2, clamped), `front`/`large` the hit geometry/weight. `None` = no spurt:
    /// censored away at this violence level (every row's column is `0` at violence 0), or a key
    /// that names no row. The `<= 0` guard below is defensive, not a "bloodless model" case:
    /// 1850 found that no shipped display resolves to one.
    pub fn effect_id(
        &self,
        blood_id: i32,
        violence: usize,
        front: bool,
        large: bool,
    ) -> Option<u32> {
        let row = self.row(blood_id, violence)?;
        let idx = match (front, large) {
            (true, false) => 0,
            (true, true) => 1,
            (false, false) => 2,
            (false, true) => 3,
        };
        Some(row.effects[idx]).filter(|&id| id != 0)
    }

    /// The **ground-splat decal textures** for a victim — the same row resolve as
    /// [`Self::effect_id`] (`blood id → UnitBloodLevels[violence] → UnitBlood row`), so a censored
    /// violence level drops the splat exactly as it drops the spurt. The slice is the row's non-empty texture columns in column order: extensionless
    /// `textures\BloodSplats\…` paths, four per row in the shipped table. Empty = no splat.
    pub fn splats(&self, blood_id: i32, violence: usize) -> &[String] {
        self.row(blood_id, violence).map_or(&[], |r| &r.splats)
    }

    /// The victim's resolved `UnitBlood` row: the [`Self::level_key`] blood id keyed through the
    /// violence-level censorship table (`GetUnitBloodRecord 0x60a390`). `None` for a violence
    /// level this creature yields no blood at, for a row the table doesn't carry, or — the
    /// defensive arm — for a non-positive key no resolve produces.
    fn row(&self, blood_id: i32, violence: usize) -> Option<&BloodRow> {
        if blood_id <= 0 {
            return None;
        }
        let level = self.levels.get(&(blood_id as u32))?[violence.min(2)];
        if level == 0 {
            return None;
        }
        self.rows.get(&level)
    }

    /// Row counts (levels, rows) for logging/diagnostics.
    pub fn len(&self) -> (usize, usize) {
        (self.levels.len(), self.rows.len())
    }
}

/// UnitBloodLevels.dbc — 4 fields in build 5875: ID + the three per-violence-level bloodIDs.
fn unit_blood_levels_schema() -> Schema {
    let mut s = Schema::new("UnitBloodLevels");
    for name in ["ID", "Violence0", "Violence1", "Violence2"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s
}

/// UnitBlood.dbc — 10 fields in build 5875: ID, the four spurt `SpellVisualEffectName` ids, and
/// five ground-splat texture strings (four named + one empty in every shipped row).
fn unit_blood_schema() -> Schema {
    let mut s = Schema::new("UnitBlood");
    for name in ["ID", "FrontSmall", "FrontLarge", "BackSmall", "BackLarge"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    for i in 0..5 {
        s.add_field(SchemaField::new(
            format!("GroundSplat{i}"),
            FieldType::String,
        ));
    }
    s
}

/// Load both blood tables from the patch chain into a [`BloodCatalog`].
pub fn load_blood_catalog(chain: &mut Chain) -> Result<BloodCatalog> {
    let (levels, default_level) = {
        let bytes = chain
            .read_file(UNIT_BLOOD_LEVELS)
            .with_context(|| format!("reading {UNIT_BLOOD_LEVELS}"))?;
        let rs = parse(&bytes, unit_blood_levels_schema(), "UnitBloodLevels")?;
        // File order matters here and nowhere else: tier 3 returns the RECORDS BASE, so the
        // fallback id is row 0's, read before the rows go into the (unordered) map.
        let default_level = rs.records().first().and_then(|r| u32_at(r, 0));
        let mut m = HashMap::with_capacity(rs.records().len());
        for r in rs.records() {
            if let Some(id) = u32_at(r, 0) {
                m.insert(
                    id,
                    [
                        u32_at(r, 1).unwrap_or(0),
                        u32_at(r, 2).unwrap_or(0),
                        u32_at(r, 3).unwrap_or(0),
                    ],
                );
            }
        }
        (m, default_level)
    };
    let rows = {
        let bytes = chain
            .read_file(UNIT_BLOOD)
            .with_context(|| format!("reading {UNIT_BLOOD}"))?;
        let rs = parse(&bytes, unit_blood_schema(), "UnitBlood")?;
        let mut m = HashMap::with_capacity(rs.records().len());
        for r in rs.records() {
            if let Some(id) = u32_at(r, 0) {
                m.insert(
                    id,
                    BloodRow {
                        effects: [
                            u32_at(r, 1).unwrap_or(0),
                            u32_at(r, 2).unwrap_or(0),
                            u32_at(r, 3).unwrap_or(0),
                            u32_at(r, 4).unwrap_or(0),
                        ],
                        // `str_at` drops the empty columns, so a row keeps only the textures it
                        // actually names — the fifth column throughout the shipped table.
                        splats: (5..10).filter_map(|i| str_at(&rs, r, i)).collect(),
                    },
                );
            }
        }
        m
    };
    Ok(BloodCatalog {
        levels,
        default_level,
        rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real 5875 tables, end to end: a red-blooded creature (blood 1 — HumanMale's) at max
    /// violence spurts RED (effect 109 front-small = `Particles\BloodSpurts\BloodSpurt.mdl`,
    /// 55 back-large); at violence 1 the same creature is CENSORED to green (row 2's ids); at 0
    /// nothing. Pins the empirically-derived column order against the shipped data.
    #[test]
    fn real_blood_tables_resolve_the_censorship_chain() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let blood = load_blood_catalog(&mut chain).expect("blood tables");

        // Max violence: true colors, all four quadrants of row 1 (red).
        assert_eq!(
            blood.effect_id(1, 2, true, false),
            Some(109),
            "front small red"
        );
        assert_eq!(
            blood.effect_id(1, 2, true, true),
            Some(164),
            "front large red"
        );
        assert_eq!(
            blood.effect_id(1, 2, false, false),
            Some(534),
            "back small red"
        );
        assert_eq!(
            blood.effect_id(1, 2, false, true),
            Some(55),
            "back large red"
        );
        // Violence 1: red is censored to green (UnitBloodLevels row 1 → bloodID 2).
        assert_eq!(
            blood.effect_id(1, 1, true, false),
            Some(183),
            "censored green"
        );
        // Violence 0: no blood at all; the defensive non-positive guard yields nothing either.
        assert_eq!(blood.effect_id(1, 0, true, false), None);
        assert_eq!(blood.effect_id(-1, 2, true, false), None);
        assert_eq!(blood.effect_id(0, 2, true, false), None);
    }

    /// The **three-tier row resolve** (`0x60afb0`), the bug 1850 fixed: a display whose
    /// `BloodLevel` is `0` and whose model's `BloodID` is `−1` is NOT bloodless — both tiers miss
    /// and tier 3 hands back UnitBloodLevels' file row 0, id 1, the RED row. Pinned end to end
    /// against the shipped creature tables, including the population that reaches each tier.
    #[test]
    fn the_blood_row_resolve_falls_through_to_the_records_base() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let blood = load_blood_catalog(&mut chain).expect("blood tables");

        // Tier 1 wins when the display carries a real override; tier 2 when only the model does.
        assert_eq!(
            blood.level_key(3, 1),
            Some(3),
            "tier 1: the display override"
        );
        assert_eq!(
            blood.level_key(0, 2),
            Some(2),
            "tier 2: 0 is not a row of the table"
        );
        // Tier 3 — the 595-display case. `-1` is a MISS, not a bloodless marker.
        assert_eq!(
            blood.level_key(0, -1),
            Some(1),
            "tier 3: the records base, id 1 (red)"
        );
        assert_eq!(
            blood.level_key(-1, -1),
            Some(1),
            "tier 3: both tiers negative"
        );
        assert_eq!(
            blood.level_key(99, 99),
            Some(1),
            "tier 3: both tiers out of range"
        );
        // …and a tier-3 unit really does spurt, which is exactly what benilla dropped before.
        assert_eq!(blood.effect_id(1, 2, true, false), Some(109));
    }

    /// The ground-splat columns of the same three rows: **four** textures each, colour-matched to
    /// the row the violence level selects — red at max, the censored green one rung down, nothing
    /// at 0 — and the fifth column empty throughout, so the shipped slice is length 4, never 5.
    #[test]
    fn real_blood_tables_carry_four_ground_splats_per_row() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let blood = load_blood_catalog(&mut chain).expect("blood tables");

        assert_eq!(
            blood.splats(1, 2),
            [
                r"textures\BloodSplats\BloodSplatRed01",
                r"textures\BloodSplats\BloodSplatRed02",
                r"textures\BloodSplats\BloodSplatRed03",
                r"textures\BloodSplats\BloodSplatRed04",
            ],
            "red blood at max violence"
        );
        // Censored one rung down: the same creature's splats go green (row 2), matching the spurt.
        assert!(blood.splats(1, 1).iter().all(|p| p.contains("Green")));
        assert_eq!(blood.splats(1, 1).len(), 4);
        // Row 3 is the black-blood row; violence 0 (and the defensive guard) splat nothing.
        assert!(blood.splats(3, 2).iter().all(|p| p.contains("Black")));
        assert!(blood.splats(1, 0).is_empty());
        assert!(blood.splats(-1, 2).is_empty());
        assert!(blood.splats(0, 2).is_empty());
    }
}
