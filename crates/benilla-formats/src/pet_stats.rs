//! `PetPersonality.dbc` + `PetLoyalty.dbc` — the two tables behind a hunter pet's happiness and
//! loyalty readouts (decision 1005).
//!
//! **`GetPetHappiness` does its own thresholding.** The client does not hand Lua a raw happiness
//! number for the UI to bucket — it returns a **pre-bucketed 1/2/3** plus the two numbers that
//! bucket implies, from a three-column-triple row (`0x4be947`–`0x4be9c3`):
//!
//! ```text
//! rec = <table>[personalityId]           ; missing/out-of-range -> FALLBACK rec = <table>[1]
//! raw = UNIT_FIELD_POWER5                ; the happiness power
//! esi = 0; while (esi < 3 && raw >= rec[0x28 + 4*esi]) esi++      ; the three thresholds
//! ret1 = esi                             ; 0..3
//! ret2 = rec[0x30 + 4*esi] * 100.0f      ; damage percentage   (esi >= 1)
//! ret3 = rec[0x3c + 4*esi]               ; loyalty rate        (esi >= 1, may be NEGATIVE)
//! ```
//!
//! **Which `.dbc` backs it is NOT established from the bytes** — `[0xc0d9e0]`/`[0xc0d9e4]` is
//! known only as an anonymous BSS `{indexTable, maxId}` pair. The index is not a *creature
//! family*, and the file settles it: `CreatureFamily.dbc`
//! has a **0x48-byte** record — the read at `rec+0x48` would run off the end — and every dword from
//! `0x28` up is zero in all 23 rows. `PetPersonality.dbc` is a 0x4c-byte record whose last nine
//! columns are exactly the three triples, at exactly those offsets:
//!
//! | id | thresholds `0x28` | damage `0x34` | loyalty rate `0x40` |
//! |---|---|---|---|
//! | 1 | 0 / 333000 / 666000 | 0.75 / 1.00 / 1.25 | −10 / 5 / 20 |
//! | 3 | 0 / 250000 / 750000 | 0.00 / 1.00 / 1.25 | −1 / 0 / 2 |
//!
//! Row 1 is vanilla's documented pet-happiness behaviour on the nose — unhappy pets deal 75%
//! damage, content 100%, happy 125%, against a happiness power that runs 0…1,000,000 in thirds —
//! which is the independent corroboration that this is the table and row 1 is the row live pets
//! use. (See [`PetPersonalities::for_pet`] for what is still open: *which field* selects the row.)
//!
//! `PetLoyalty.dbc` is the plain half: 8 rows, `ID` + the localized `Name` block, read at
//! `[[0xc0d9f4][lvl] + 4*locale + 4]` — i.e. `Name` enUS is field 1, the same shape every other
//! localized table here uses.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::chain::Chain;
use crate::dbc::{f32_at, parse, str_at, u32_at};

const PET_PERSONALITY: &str = "DBFilesClient\\PetPersonality.dbc";
const PET_LOYALTY: &str = "DBFilesClient\\PetLoyalty.dbc";

/// `PetPersonality.dbc`'s column count (the DBC header's `field_count`; `benilla-dbc` enforces it).
const PERSONALITY_FIELDS: usize = 19;
/// `PetLoyalty.dbc`'s column count.
const LOYALTY_FIELDS: usize = 10;
/// The localized `Name` block's enUS column — field 1 in both files, and the `+4` in the client's
/// own `[row + 4*locale + 4]`.
const NAME_FIELD: usize = 1;

/// The first threshold column: byte `0x28` = field 10.
const THRESHOLD_FIELD: usize = 0x28 / 4;
/// The first damage-percentage column: byte `0x34` = field 13.
const DAMAGE_FIELD: usize = 0x34 / 4;
/// The first loyalty-rate column: byte `0x40` = field 16.
const LOYALTY_RATE_FIELD: usize = 0x40 / 4;

/// The personality id the client falls back to when it cannot resolve one (`0x4be96c`: `rec` null
/// or the index out of range ⇒ `rec = indexTable[1]`). Not a benilla convention — the reference's
/// own second chance, and the row every live pet is observed to use.
pub const FALLBACK_PERSONALITY: u32 = 1;

/// One `PetPersonality` row's three parallel triples, indexed by the happiness bucket.
///
/// The triples are **1-based against the bucket**: bucket 1 reads slot 0. Bucket 0 never indexes
/// them at all — the client jumps straight to the shared `(100.0, 0.0)` tail — which is why
/// [`PetHappiness`] carries the numbers rather than the caller doing the offset arithmetic.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PetPersonality {
    /// The three ascending happiness thresholds the raw power is counted against.
    pub thresholds: [u32; 3],
    /// Damage multiplier per bucket, as stored (`0.75` = 75%). The binding scales by 100.
    pub damage: [f32; 3],
    /// Loyalty gain rate per bucket. **May be negative** — an unhappy pet loses loyalty.
    pub loyalty_rate: [f32; 3],
}

/// What `GetPetHappiness` answers: the bucket plus the two numbers it selects.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PetHappiness {
    /// `0..=3`. **`0` is not the failure case** — the client pushes it as the number `0` and takes
    /// the same tail as a gate failure, and the shipped `PetFrame.lua` has no branch for it, so the
    /// icon keeps whatever texcoords it had. Keep it distinct from "no answer at all".
    pub bucket: u32,
    /// Return 2 — the damage percentage, already scaled by the client's own `100.0f`
    /// (`[0x806b10]`). `100.0` for bucket 0.
    pub damage_percentage: f32,
    /// Return 3 — the loyalty rate, unscaled and possibly negative. `0.0` for bucket 0.
    pub loyalty_rate: f32,
}

impl PetPersonality {
    /// Bucket a raw happiness power and pick the row's two numbers — `0x4be981`'s count loop and
    /// the two indexed reads after it, transcribed.
    pub fn happiness(&self, raw: u32) -> PetHappiness {
        let bucket = self.thresholds.iter().take_while(|&&t| raw >= t).count();
        // Bucket 0 shares the gate-failure tail (`0x4be9a8 je 0x4be9e9`) rather than indexing.
        let Some(i) = bucket.checked_sub(1) else {
            return PetHappiness {
                bucket: 0,
                damage_percentage: 100.0,
                loyalty_rate: 0.0,
            };
        };
        PetHappiness {
            bucket: bucket as u32,
            damage_percentage: self.damage[i] * 100.0,
            loyalty_rate: self.loyalty_rate[i],
        }
    }
}

/// `PetPersonality.dbc`, by id.
pub struct PetPersonalities(HashMap<u32, PetPersonality>);

impl PetPersonalities {
    /// The row for a pet whose personality id is `id`, with the client's own fallback applied.
    ///
    /// **`id` is `None` today, always, and that is a recorded gap rather than an oversight.** The
    /// client selects the row with `0x605600(pet)` = `[[pet+0xb30]+0x24]` — a field of its cached
    /// creature template — and which template field `+0x24` is, and which table it indexes, are
    /// both open. Passing `None` takes the reference's *own* out-of-range path
    /// ([`FALLBACK_PERSONALITY`]), which is what every live pet is observed to land on: the file
    /// ships two rows, ids 1 and 3, and row 1 alone reproduces vanilla's 75/100/125% damage.
    ///
    /// So the shape is right and the numbers are right; when the selector is identified, this call
    /// gains an argument and nothing else moves.
    pub fn for_pet(&self, id: Option<u32>) -> Option<&PetPersonality> {
        id.and_then(|i| self.0.get(&i))
            .or_else(|| self.0.get(&FALLBACK_PERSONALITY))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// `PetLoyalty.dbc`: loyalty level → its localized name.
pub struct PetLoyaltyNames(HashMap<u32, String>);

impl PetLoyaltyNames {
    /// The name for a loyalty level, or `None` — which is `GetPetLoyalty`'s **nil**, and it is nil
    /// for level `0` as well as for anything past the table (`0x4be700`'s bound against
    /// `[0xc0d9f8]`, plus `lua_pushstring`'s own NULL→nil).
    pub fn name(&self, level: u32) -> Option<&str> {
        (level != 0).then(|| self.0.get(&level).map(String::as_str))?
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn personality_schema() -> Schema {
    let mut s = Schema::new("PetPersonality");
    for i in 0..PERSONALITY_FIELDS {
        let ty = match i {
            NAME_FIELD => FieldType::String,
            i if (DAMAGE_FIELD..DAMAGE_FIELD + 3).contains(&i) => FieldType::Float32,
            i if (LOYALTY_RATE_FIELD..LOYALTY_RATE_FIELD + 3).contains(&i) => FieldType::Float32,
            // ID, the rest of the localization block, and the three thresholds (which the client
            // compares as integers against an integer power field).
            _ => FieldType::UInt32,
        };
        s.add_field(SchemaField::new(format!("F{i}"), ty));
    }
    s
}

fn loyalty_schema() -> Schema {
    let mut s = Schema::new("PetLoyalty");
    for i in 0..LOYALTY_FIELDS {
        let ty = if i == NAME_FIELD {
            FieldType::String
        } else {
            FieldType::UInt32
        };
        s.add_field(SchemaField::new(format!("F{i}"), ty));
    }
    s
}

/// Load `PetPersonality.dbc` from the patch chain.
pub fn load_pet_personalities(chain: &mut Chain) -> Result<PetPersonalities> {
    let bytes = chain
        .read_file(PET_PERSONALITY)
        .with_context(|| format!("reading {PET_PERSONALITY}"))?;
    let rs = parse(&bytes, personality_schema(), "PetPersonality.dbc")?;
    let mut by_id = HashMap::new();
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let triple_u32 = |base: usize| [0, 1, 2].map(|i| u32_at(r, base + i).unwrap_or(0));
        let triple_f32 = |base: usize| [0, 1, 2].map(|i| f32_at(r, base + i).unwrap_or(0.0));
        by_id.insert(
            id,
            PetPersonality {
                thresholds: triple_u32(THRESHOLD_FIELD),
                damage: triple_f32(DAMAGE_FIELD),
                loyalty_rate: triple_f32(LOYALTY_RATE_FIELD),
            },
        );
    }
    Ok(PetPersonalities(by_id))
}

/// Load `PetLoyalty.dbc` from the patch chain.
pub fn load_pet_loyalty_names(chain: &mut Chain) -> Result<PetLoyaltyNames> {
    let bytes = chain
        .read_file(PET_LOYALTY)
        .with_context(|| format!("reading {PET_LOYALTY}"))?;
    let rs = parse(&bytes, loyalty_schema(), "PetLoyalty.dbc")?;
    let mut by_id = HashMap::new();
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        if let Some(name) = str_at(&rs, r, NAME_FIELD) {
            by_id.insert(id, name);
        }
    }
    Ok(PetLoyaltyNames(by_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain() -> Option<crate::chain::Chain> {
        let data = crate::wow_data_or_skip!(None);
        Some(crate::open_chain(&data).expect("open chain"))
    }

    /// The real 5875 `PetPersonality.dbc`, byte-anchored. A column slip here is silent and
    /// catastrophic — the thresholds and the damage triple are adjacent, so reading one for the
    /// other yields plausible-looking numbers and a pet that is permanently "unhappy".
    #[test]
    fn the_real_personality_rows_carry_vanillas_own_happiness_numbers() {
        let Some(mut chain) = chain() else { return };
        let t = load_pet_personalities(&mut chain).expect("load PetPersonality.dbc");
        assert_eq!(
            t.len(),
            2,
            "5875 ships exactly two personalities, ids 1 and 3"
        );

        let one = t.for_pet(Some(1)).expect("id 1");
        assert_eq!(one.thresholds, [0, 333_000, 666_000]);
        assert_eq!(one.damage, [0.75, 1.0, 1.25]);
        assert_eq!(one.loyalty_rate, [-10.0, 5.0, 20.0]);

        let three = t.for_pet(Some(3)).expect("id 3");
        assert_eq!(three.thresholds, [0, 250_000, 750_000]);

        // The client's own fallback, and the reason it is load-bearing for us: an unresolved
        // personality is the ONLY path we take today, and it must land on row 1.
        assert_eq!(t.for_pet(None), t.for_pet(Some(1)));
        assert_eq!(t.for_pet(Some(999)), t.for_pet(Some(1)));
    }

    /// The bucket loop against the shipped row 1, at the boundaries — and the ×100 scaling, which
    /// is the client's and not the UI's.
    #[test]
    fn the_bucket_loop_counts_thresholds_met() {
        let Some(mut chain) = chain() else { return };
        let t = load_pet_personalities(&mut chain).expect("load");
        let p = *t.for_pet(None).expect("fallback row");

        // Happiness runs 0..1_000_000 in thirds. Every reachable raw value buckets 1..3.
        for (raw, bucket, dmg) in [
            (0, 1, 75.0),
            (332_999, 1, 75.0),
            (333_000, 2, 100.0),
            (665_999, 2, 100.0),
            (666_000, 3, 125.0),
            (1_000_000, 3, 125.0),
        ] {
            let h = p.happiness(raw);
            assert_eq!((h.bucket, h.damage_percentage), (bucket, dmg), "raw {raw}");
        }
        assert_eq!(
            p.happiness(0).loyalty_rate,
            -10.0,
            "an unhappy pet LOSES loyalty"
        );
        assert_eq!(p.happiness(1_000_000).loyalty_rate, 20.0);
    }

    /// Bucket 0 is structurally reachable (a row whose first threshold is above the raw value) and
    /// is NOT the failure case: it answers the number `0` with the same `(100.0, 0.0)` tail a gate
    /// failure uses. A re-implementation that folded it into nil would hide the pet frame.
    #[test]
    fn bucket_zero_is_a_number_not_a_failure() {
        let p = PetPersonality {
            thresholds: [10, 20, 30],
            damage: [0.75, 1.0, 1.25],
            loyalty_rate: [-10.0, 5.0, 20.0],
        };
        let h = p.happiness(9);
        assert_eq!(h.bucket, 0);
        assert_eq!((h.damage_percentage, h.loyalty_rate), (100.0, 0.0));
    }

    /// The eight real loyalty names, verbatim, and the two nil cases the binding must reproduce.
    ///
    /// **The shipped strings carry a `"(Loyalty Level N) "` prefix** — developer annotation left in
    /// the data, and the client pushes the column with no stripping whatsoever
    /// (`lua_pushstring` of `[row + 4*locale + 4]`). So that prefix is what the pet paper doll
    /// shows, and trimming it here would be a "tidier than the reference" divergence. Levels 7 and
    /// 8 are `"Loyalty Cap"` and `"Unused"`: the ladder players actually climb is 1–6.
    #[test]
    fn the_real_loyalty_levels_are_named_verbatim() {
        let Some(mut chain) = chain() else { return };
        let n = load_pet_loyalty_names(&mut chain).expect("load PetLoyalty.dbc");
        assert_eq!(n.len(), 8);
        assert_eq!(n.name(1), Some("(Loyalty Level 1) Rebellious"));
        assert_eq!(n.name(3), Some("(Loyalty Level 3) Submissive"));
        assert_eq!(n.name(6), Some("(Loyalty Level 6) Best Friend"));
        assert_eq!(n.name(7), Some("(Loyalty Level 7) Loyalty Cap"));
        assert_eq!(n.name(8), Some("(Loyalty Level 8) Unused"));
        // Level 0 is "no loyalty yet" and the client answers nil, not the first row.
        assert_eq!(n.name(0), None);
        assert_eq!(n.name(9), None);
    }
}
