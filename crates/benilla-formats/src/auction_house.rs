//! AuctionHouse.dbc — the **rates** each auction house charges: the deposit it takes to list an
//! item, and the cut it takes out of a sale.
//!
//! One consumer, and it is why the table has to be loaded rather than hardcoded: the sell pane's
//! `Deposit:` line has to show a number *before* the item is listed, so the client computes it
//! locally — `GetAuctionHouseDepositRate()` is a straight read of [`AuctionHouseCatalog::deposit_percent`]
//! at the house the session is talking to. Which house that is arrives on the wire:
//! `MSG_AUCTION_HELLO`'s reply carries a `houseId` alongside the auctioneer guid, and it keys this
//! table. Send the window the wrong id and the pane quietly shows Blackwater's 25%
//! where a city auctioneer charges 5%.
//!
//! Record layout (7 rows in the shipped 5875 file, verified by reading it): `ID@0`, `FactionID@1`,
//! `DepositPercent@2`, `CutPercent@3`, `Name_Lang@4..11`, `NameFlags@12`. Ids are 1..=7 and
//! contiguous, but this is keyed rather than indexed for the reason every catalog here is keyed: a
//! `houseId` off the wire is data, not an index we control.
//!
//! The shipped table is two tiers — the six faction houses charge **5% / 5%**, and Blackwater
//! (id 7, the neutral goblin house at faction 369) charges **25% / 15%**. That spread is the whole
//! economic point of the neutral auction house, so it is data worth reading rather than a constant
//! worth inlining.
//!
//! **The rate is only half of the deposit.** The server's charge also scales by stack count, by
//! duration, and by its own `Rate.Auction.Deposit` config multiplier, none of which live here; and
//! the real client's displayed number is computed with an intermediate truncation the server's is
//! not, so the two can disagree by a copper or two on cheap items. Decision 1511 records that
//! divergence and what benilla does about it — this module only supplies the percentage.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const AUCTION_HOUSE: &str = "DBFilesClient\\AuctionHouse.dbc";

/// One auction house's row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuctionHouseInfo {
    /// The faction this house serves (`369` = Booty Bay, the neutral house).
    pub faction: u32,
    /// Percent of the item's vendor sell price taken to list it, before stack/duration scaling.
    pub deposit_percent: u32,
    /// Percent of the winning bid taken out of the seller's proceeds.
    pub cut_percent: u32,
    /// The house's name ("Stormwind Auction House"), as shipped.
    pub name: String,
}

/// AuctionHouse.dbc keyed by the `houseId` that `MSG_AUCTION_HELLO`'s reply carries.
pub struct AuctionHouseCatalog {
    houses: HashMap<u32, AuctionHouseInfo>,
}

impl AuctionHouseCatalog {
    /// The whole row for `house_id`; `None` for an id with no row.
    pub fn get(&self, house_id: u32) -> Option<&AuctionHouseInfo> {
        self.houses.get(&house_id)
    }

    /// The listing deposit rate, in percent — `GetAuctionHouseDepositRate()`'s answer. `None` for
    /// an id with no row, which leaves the caller to decide (the sell pane shows no deposit rather
    /// than inventing one).
    pub fn deposit_percent(&self, house_id: u32) -> Option<u32> {
        self.houses.get(&house_id).map(|h| h.deposit_percent)
    }

    /// The sale cut, in percent.
    pub fn cut_percent(&self, house_id: u32) -> Option<u32> {
        self.houses.get(&house_id).map(|h| h.cut_percent)
    }

    /// Row count, for the load log.
    pub fn len(&self) -> usize {
        self.houses.len()
    }

    pub fn is_empty(&self) -> bool {
        self.houses.is_empty()
    }
}

fn auction_house_schema() -> Schema {
    let mut s = Schema::new("AuctionHouse");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("FactionID", FieldType::UInt32));
    s.add_field(SchemaField::new("DepositPercent", FieldType::UInt32));
    s.add_field(SchemaField::new("CutPercent", FieldType::UInt32));
    for i in 0..8 {
        s.add_field(SchemaField::new(format!("Name{i}"), FieldType::String));
    }
    s.add_field(SchemaField::new("NameFlags", FieldType::UInt32));
    s
}

/// Load AuctionHouse.dbc from the patch chain.
pub fn load_auction_houses(chain: &mut Chain) -> Result<AuctionHouseCatalog> {
    let bytes = chain
        .read_file(AUCTION_HOUSE)
        .with_context(|| format!("reading {AUCTION_HOUSE}"))?;
    let rs = parse(&bytes, auction_house_schema(), "AuctionHouse")?;
    let mut houses = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let (Some(id), Some(faction), Some(deposit_percent), Some(cut_percent)) =
            (u32_at(r, 0), u32_at(r, 1), u32_at(r, 2), u32_at(r, 3))
        else {
            continue;
        };
        houses.insert(
            id,
            AuctionHouseInfo {
                faction,
                deposit_percent,
                cut_percent,
                name: str_at(&rs, r, 4).unwrap_or_default(),
            },
        );
    }
    Ok(AuctionHouseCatalog { houses })
}

#[cfg(test)]
mod tests {
    use super::load_auction_houses;

    /// The shipped 5875 table, read as data. The two tiers pinned here are the arithmetic the sell
    /// pane rests on: every faction house is 5%/5%, and the neutral goblin house is 25%/15% — the
    /// spread that makes cross-faction trading cost something. Skips without client data.
    #[test]
    fn the_shipped_houses_carry_the_two_rate_tiers() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_auction_houses(&mut chain).expect("AuctionHouse.dbc");

        assert_eq!(cat.len(), 7, "the whole shipped table");

        // The six faction houses: a flat 5% to list, 5% of the sale.
        for id in 1..=6 {
            let h = cat.get(id).unwrap_or_else(|| panic!("house {id}"));
            assert_eq!(h.deposit_percent, 5, "house {id} deposit");
            assert_eq!(h.cut_percent, 5, "house {id} cut");
        }
        assert_eq!(
            cat.get(1).map(|h| h.name.as_str()),
            Some("Stormwind Auction House")
        );

        // Blackwater — the neutral house, and the reason this is a table and not a constant.
        let neutral = cat.get(7).expect("house 7");
        assert_eq!(neutral.faction, 369, "Booty Bay");
        assert_eq!(neutral.deposit_percent, 25);
        assert_eq!(neutral.cut_percent, 15);
        assert_eq!(neutral.name, "Blackwater Auction House");

        // Past the table: no row. The sell pane shows no deposit rather than inventing one.
        assert_eq!(cat.deposit_percent(8), None);
    }
}
