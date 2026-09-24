//! `PetPersonality.dbc` and `PetLoyalty.dbc`, behind a hunter pet's happiness and loyalty
//! readouts. `GetPetHappiness` buckets on its own (`0x4be947`-`0x4be9c3`), counting the row's
//! thresholds the happiness power (`UNIT_FIELD_POWER5`) meets. Its store, `[0xc0d9e0]`/`[0xc0d9e4]`
//! (rows and max id), is `PetPersonality.dbc` (loader `0x54bce0`), whose `0x4c`-byte rows end in
//! the three triples; `GetPetLoyalty` reads `PetLoyalty.dbc` at `[[0xc0d9f4][lvl] + 4*locale + 4]`.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::chain::Chain;
use crate::dbc::{f32_at, parse, str_at, u32_at};

const PET_PERSONALITY: &str = "DBFilesClient\\PetPersonality.dbc";
const PET_LOYALTY: &str = "DBFilesClient\\PetLoyalty.dbc";

/// `PetPersonality.dbc`'s column count, which `benilla-dbc` checks against the header.
const PERSONALITY_FIELDS: usize = 19;
const LOYALTY_FIELDS: usize = 10;
/// The enUS `Name` column in both files, the `+4` in the client's `[row + 4*locale + 4]`.
const NAME_FIELD: usize = 1;

const THRESHOLD_FIELD: usize = 0x28 / 4;
const DAMAGE_FIELD: usize = 0x34 / 4;
const LOYALTY_RATE_FIELD: usize = 0x40 / 4;

/// The personality the client falls back to when the id is out of range or its row is missing
/// (`0x4be96c`), "Personality: Standard".
pub const FALLBACK_PERSONALITY: u32 = 1;

/// One `PetPersonality` row's triples, read 1-based: bucket 1 reads slot 0, bucket 0 reads none.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PetPersonality {
    /// The three ascending happiness thresholds the raw power is counted against.
    pub thresholds: [u32; 3],
    /// Damage multiplier per bucket as stored (`0.75` is 75%).
    pub damage: [f32; 3],
    /// Loyalty gain rate per bucket, negative for an unhappy pet.
    pub loyalty_rate: [f32; 3],
}

/// What `GetPetHappiness` answers: the bucket plus the two numbers it selects.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PetHappiness {
    /// `0..=3`. `0` is a number, not the failure case: the client pushes `0` with the failure's
    /// tail, and `PetFrame.lua`, with no branch for it, keeps the icon's old texcoords.
    pub bucket: u32,
    /// The damage percentage, times the client's `100.0f` (`[0x806b10]`); `100.0` for bucket 0.
    pub damage_percentage: f32,
    /// The loyalty rate, unscaled and possibly negative; `0.0` for bucket 0.
    pub loyalty_rate: f32,
}

impl PetPersonality {
    /// Bucket a raw happiness power and pick the row's two numbers (`0x4be981`).
    pub fn happiness(&self, raw: u32) -> PetHappiness {
        let bucket = self.thresholds.iter().take_while(|&&t| raw >= t).count();
        // Bucket 0 takes the gate-failure tail (`0x4be9a8 je 0x4be9e9`) instead of indexing.
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
    /// The row for personality `id`, or the client's fallback row. The reference keys this table
    /// by `[[pet+0xb30]+0x24]` (`0x605600`), a field of the cached creature-query record whose
    /// wire source is untraced, so callers pass `None` and get row 1.
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

/// `PetLoyalty.dbc`: loyalty level to its localized name.
pub struct PetLoyaltyNames(HashMap<u32, String>);

impl PetLoyaltyNames {
    /// The name for a loyalty level. `None` is `GetPetLoyalty`'s nil, for level `0` and past the
    /// table (`0x4be700` bounds it against `[0xc0d9f8]`).
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
            // ID, the rest of the name block, and the thresholds, compared as integers.
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

        // The client's fallback, the path every caller takes, lands on row 1.
        assert_eq!(t.for_pet(None), t.for_pet(Some(1)));
        assert_eq!(t.for_pet(Some(999)), t.for_pet(Some(1)));
    }

    #[test]
    fn the_bucket_loop_counts_thresholds_met() {
        let Some(mut chain) = chain() else { return };
        let t = load_pet_personalities(&mut chain).expect("load");
        let p = *t.for_pet(None).expect("fallback row");

        // Happiness runs 0..1_000_000 in thirds, so every reachable value buckets 1..3.
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

    /// Bucket 0 is reachable only through a row whose first threshold is above the raw value.
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

    /// The paper doll shows the shipped `"(Loyalty Level N) "` prefix: the client never strips it.
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
