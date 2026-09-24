//! `AuctionHouse.dbc`, each auction house's deposit and cut rates. The server scales the deposit
//! further by stack count, duration and `Rate.Auction.Deposit`, and the reference's displayed
//! figure truncates midway where the server's does not, so they can differ by a copper or two.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const AUCTION_HOUSE: &str = "DBFilesClient\\AuctionHouse.dbc";

/// One auction house's row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuctionHouseInfo {
    /// The faction this house serves (`369`, Booty Bay, for the neutral house).
    pub faction: u32,
    /// Percent of the item's vendor sell price taken to list it, before stack and duration scaling.
    pub deposit_percent: u32,
    /// Percent of the winning bid taken out of the seller's proceeds.
    pub cut_percent: u32,
    /// The house's name (`"Stormwind Auction House"`).
    pub name: String,
}

/// `AuctionHouse.dbc` keyed by the `houseId` that `MSG_AUCTION_HELLO`'s reply carries.
pub struct AuctionHouseCatalog {
    houses: HashMap<u32, AuctionHouseInfo>,
}

impl AuctionHouseCatalog {
    /// The row for `house_id`.
    pub fn get(&self, house_id: u32) -> Option<&AuctionHouseInfo> {
        self.houses.get(&house_id)
    }

    /// The listing deposit rate in percent, `GetAuctionHouseDepositRate()`'s answer.
    pub fn deposit_percent(&self, house_id: u32) -> Option<u32> {
        self.houses.get(&house_id).map(|h| h.deposit_percent)
    }

    /// The sale cut in percent.
    pub fn cut_percent(&self, house_id: u32) -> Option<u32> {
        self.houses.get(&house_id).map(|h| h.cut_percent)
    }

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

/// Load `AuctionHouse.dbc` from the patch chain.
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

    #[test]
    fn the_shipped_houses_carry_the_two_rate_tiers() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_auction_houses(&mut chain).expect("AuctionHouse.dbc");

        assert_eq!(cat.len(), 7, "the whole shipped table");

        for id in 1..=6 {
            let h = cat.get(id).unwrap_or_else(|| panic!("house {id}"));
            assert_eq!(h.deposit_percent, 5, "house {id} deposit");
            assert_eq!(h.cut_percent, 5, "house {id} cut");
        }
        assert_eq!(
            cat.get(1).map(|h| h.name.as_str()),
            Some("Stormwind Auction House")
        );

        let neutral = cat.get(7).expect("house 7");
        assert_eq!(neutral.faction, 369, "Booty Bay");
        assert_eq!(neutral.deposit_percent, 25);
        assert_eq!(neutral.cut_percent, 15);
        assert_eq!(neutral.name, "Blackwater Auction House");

        assert_eq!(cat.deposit_percent(8), None);
    }
}
