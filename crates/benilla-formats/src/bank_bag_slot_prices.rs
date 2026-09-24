//! `BankBagSlotPrices.dbc`: the copper price of each bank bag slot. Rows 7-12 hold an unreachable
//! `999999999` sentinel, as `GetNumBankSlots()` reports full at 6. `CMSG_BUY_BANK_SLOT` carries no
//! index: the server buys the slot after the count in `PLAYER_BYTES_2` byte 2.

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};
use std::collections::HashMap;

use crate::dbc::{parse, u32_at};
use crate::Chain;

const BANK_BAG_SLOT_PRICES: &str = "DBFilesClient\\BankBagSlotPrices.dbc";

/// Row id (1-based slot number) → price in copper.
pub struct BankBagSlotPrices(HashMap<u32, u32>);

impl BankBagSlotPrices {
    /// The price of slot `purchased_count + 1`; a sentinel row still reads as its price.
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
    let mut s = Schema::new("BankBagSlotPrices");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("Price", FieldType::UInt32));
    s.set_key_field("ID");
    s
}

/// Load `BankBagSlotPrices.dbc` from the patch chain.
pub fn load_bank_bag_slot_prices(chain: &mut Chain) -> Result<BankBagSlotPrices> {
    let bytes = chain
        .read_file(BANK_BAG_SLOT_PRICES)
        .with_context(|| format!("reading {BANK_BAG_SLOT_PRICES}"))?;
    table_from(&bytes)
}

fn table_from(bytes: &[u8]) -> Result<BankBagSlotPrices> {
    let rs = parse(bytes, schema(), "BankBagSlotPrices")?;
    let mut prices = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        if let (Some(id), Some(price)) = (u32_at(r, 0), u32_at(r, 1)) {
            prices.insert(id, price);
        }
    }
    Ok(BankBagSlotPrices(prices))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A WDBC with the shipped rows: six prices, then six `999999999` sentinel rows.
    fn synthesize() -> Vec<u8> {
        let rows: &[[u32; 2]] = &[
            [1, 1_000],
            [2, 10_000],
            [3, 100_000],
            [4, 250_000],
            [5, 500_000],
            [6, 1_000_000],
            [7, 999_999_999],
            [8, 999_999_999],
            [9, 999_999_999],
            [10, 999_999_999],
            [11, 999_999_999],
            [12, 999_999_999],
        ];
        let mut dbc = Vec::new();
        dbc.extend_from_slice(b"WDBC");
        dbc.extend_from_slice(&(rows.len() as u32).to_le_bytes());
        dbc.extend_from_slice(&2u32.to_le_bytes()); // fields
        dbc.extend_from_slice(&8u32.to_le_bytes()); // record size (2 × u32)
        dbc.extend_from_slice(&1u32.to_le_bytes()); // string block
        for row in rows {
            for v in row {
                dbc.extend_from_slice(&v.to_le_bytes());
            }
        }
        dbc.push(0); // the string block
        dbc
    }

    #[test]
    fn next_slot_price_climbs_the_ladder_then_hits_the_sentinel() {
        let table = table_from(&synthesize()).unwrap();
        assert_eq!(table.len(), 12);
        assert_eq!(table.next_slot_price(0), Some(1_000), "first slot");
        assert_eq!(table.next_slot_price(1), Some(10_000));
        assert_eq!(table.next_slot_price(2), Some(100_000));
        assert_eq!(table.next_slot_price(3), Some(250_000));
        assert_eq!(table.next_slot_price(4), Some(500_000));
        assert_eq!(
            table.next_slot_price(5),
            Some(1_000_000),
            "6th and final slot"
        );
        assert_eq!(
            table.next_slot_price(6),
            Some(999_999_999),
            "row 7 exists but is the unreachable sentinel"
        );
        assert_eq!(table.next_slot_price(11), Some(999_999_999), "row 12");
    }

    #[test]
    fn next_slot_price_is_none_past_the_table() {
        let table = table_from(&synthesize()).unwrap();
        assert_eq!(table.next_slot_price(12), None, "no row 13");
        assert_eq!(table.next_slot_price(255), None);
    }
}
