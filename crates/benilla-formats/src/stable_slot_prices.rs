//! `StableSlotPrices.dbc` — the hunter stable's purchase-ladder price table.
//!
//! **2 rows × 2 `u32` columns** (`ID`, price in copper), read out of the real 5875 `dbc.MPQ` this
//! session: row 1 = `500` (5 silver), row 2 = `50000` (5 gold). That is the whole table — there is
//! no sentinel tail and no third row, which is the [`super::bank_bag_slot_prices`] ladder's one
//! structural difference from this one (that table ships 12 rows of which only 6 are reachable).
//!
//! **Two slots is corroborated three ways**, which is why nothing here hardcodes the cap: the DBC
//! has two rows, vmangos's `MAX_PET_STABLES` is 2 (`src/game/Objects/Pet.h:37`), and the reference
//! UI's `NUM_PET_STABLE_SLOTS` is 2 (`PetStable.lua:1`). The client's job is only to price the
//! *next* slot; `CMSG_BUY_STABLE_SLOT` carries no index at all — the server buys
//! `m_stableSlots + 1` itself (`NPCHandler.cpp:704-729`) — so a row id here IS a slot number.

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};
use std::collections::HashMap;

use crate::dbc::{parse, u32_at};
use crate::Chain;

const STABLE_SLOT_PRICES: &str = "DBFilesClient\\StableSlotPrices.dbc";

/// Row id (1-based slot number) → price in copper.
pub struct StableSlotPrices(HashMap<u32, u32>);

impl StableSlotPrices {
    /// The cost of the *next* stable slot, given `purchased_count` already bought (slot
    /// `purchased_count + 1`, 1-based to match the DBC's row ids). `None` past the table — which is
    /// the "both slots owned" state the reference reads by comparing `GetNumStableSlots()` against
    /// `NUM_PET_STABLE_SLOTS` and hiding the purchase button outright (`PetStable.lua:196-199`).
    /// The absence is a data fact, not a cap this reader knows.
    pub fn next_slot_price(&self, purchased_count: u8) -> Option<u32> {
        self.0.get(&(u32::from(purchased_count) + 1)).copied()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("StableSlotPrices");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("Price", FieldType::UInt32));
    s.set_key_field("ID");
    s
}

/// Load `StableSlotPrices.dbc` from the patch chain.
pub fn load_stable_slot_prices(chain: &mut Chain) -> Result<StableSlotPrices> {
    let bytes = chain
        .read_file(STABLE_SLOT_PRICES)
        .with_context(|| format!("reading {STABLE_SLOT_PRICES}"))?;
    table_from(&bytes)
}

/// The parse itself, split from the chain read so the golden test drives the identical path.
fn table_from(bytes: &[u8]) -> Result<StableSlotPrices> {
    let rs = parse(bytes, schema(), "StableSlotPrices")?;
    let mut prices = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        if let (Some(id), Some(price)) = (u32_at(r, 0), u32_at(r, 1)) {
            prices.insert(id, price);
        }
    }
    Ok(StableSlotPrices(prices))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-built WDBC carrying the real 5875 table, byte for byte as `dbc.MPQ` ships it
    /// (extracted and dumped this session: 37 bytes total — a 20-byte header, two 8-byte records,
    /// and a 1-byte string block holding the empty string).
    fn synthesize() -> Vec<u8> {
        let rows: &[[u32; 2]] = &[[1, 500], [2, 50_000]];
        let mut dbc = Vec::new();
        dbc.extend_from_slice(b"WDBC");
        dbc.extend_from_slice(&(rows.len() as u32).to_le_bytes());
        dbc.extend_from_slice(&2u32.to_le_bytes()); // field count
        dbc.extend_from_slice(&8u32.to_le_bytes()); // record size
        dbc.extend_from_slice(&1u32.to_le_bytes()); // string block size
        for r in rows {
            for v in r {
                dbc.extend_from_slice(&v.to_le_bytes());
            }
        }
        dbc.push(0); // the string block's lone terminator
        dbc
    }

    /// The shipped ladder, as the window prices it: a hunter who owns nothing is quoted 5 silver,
    /// one who owns a slot is quoted 5 gold, and one who owns both is quoted nothing at all.
    #[test]
    fn the_shipped_ladder_prices_each_next_slot() {
        let t = table_from(&synthesize()).expect("parse");
        assert_eq!(t.len(), 2, "5875 ships exactly two stable slots");
        assert_eq!(t.next_slot_price(0), Some(500));
        assert_eq!(t.next_slot_price(1), Some(50_000));
        // Past the table: the purchase button is hidden here, and a `None` is what says so.
        assert_eq!(t.next_slot_price(2), None);
    }

    /// The real file, when there is an install to read it from. The synthesized golden above is
    /// what the logic is tested against; this is the tripwire that the golden still *matches the
    /// shipped bytes* — a silent divergence would otherwise price the window wrong forever.
    #[test]
    fn the_golden_matches_the_installed_table() {
        let data = crate::wow_data_or_skip!();
        let mut chain = Chain::open(&data).expect("open patch chain");
        let real = load_stable_slot_prices(&mut chain).expect("load real StableSlotPrices.dbc");
        let golden = table_from(&synthesize()).expect("parse golden");
        assert_eq!(real.len(), golden.len());
        for purchased in 0..3u8 {
            assert_eq!(
                real.next_slot_price(purchased),
                golden.next_slot_price(purchased),
                "slot {} price drifted from the golden",
                purchased + 1
            );
        }
    }
}
