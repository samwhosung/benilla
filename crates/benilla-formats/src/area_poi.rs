//! `AreaPOI.dbc`, the named points of interest (inns, flight masters, dungeon entrances,
//! battleground nodes) behind the minimap's landmark blips and the world map's icons. Client-only:
//! vmangos never loads it. 339 rows of 29 columns (116 B): `ID`, `Importance`, `Icon`,
//! `FactionID`, `Pos[3]`, `ContinentID`, `Flags`, `AreaID`, then `Name` and `Description` as
//! 9-column loc-strings (enUS, 7 other locales, flags), then `WorldStateID`.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, str_at, u32_at};
use crate::Chain;

const AREA_POI: &str = "DBFilesClient\\AreaPOI.dbc";

/// One `AreaPOI.dbc` row.
#[derive(Clone, Debug)]
pub struct AreaPoi {
    /// Prominence tier, `{0, 2, 3, 4}` in 5875; the minimap ranks its landmark arrows by it.
    pub importance: u32,
    /// The map icon, a cell of `POIIcons.blp`.
    pub icon: u32,
    /// `FactionTemplate.dbc` id for a faction-tinted icon, `0` for none.
    pub faction_id: u32,
    /// World position `(x, y, z)`.
    pub pos: [f32; 3],
    /// The map `pos` is in: `0` Azeroth, `1` Kalimdor, or a battleground map.
    pub continent_id: u32,
    pub flags: u32,
    /// `AreaTable.dbc` id, or `u32::MAX` (raw `-1`) for a continent-wide POI.
    pub area_id: u32,
    /// enUS display name (locale slot 0).
    pub name: String,
    /// enUS live status text (`"In Conflict"`), empty on most rows.
    pub description: String,
    /// The `WorldState` id driving the live label and icon (a battleground node's capture
    /// state), `0` for none.
    pub world_state_id: u32,
}

/// Every `AreaPOI.dbc` row in file order, plus an id index. The order is the reference's: its
/// landmark builder `0x4a67a0` walks the table by record (`[0xc0e054]`, count `[0xc0e058]`), so
/// `GetMapLandmarkInfo(i)` indexes in DBC order.
pub struct AreaPoiCatalog {
    rows: Vec<(u32, AreaPoi)>,
    by_id: HashMap<u32, usize>,
}

impl AreaPoiCatalog {
    pub fn get(&self, id: u32) -> Option<&AreaPoi> {
        self.by_id.get(&id).map(|&i| &self.rows[i].1)
    }

    /// Every row in file order.
    pub fn rows(&self) -> impl Iterator<Item = (u32, &AreaPoi)> {
        self.rows.iter().map(|(id, poi)| (*id, poi))
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("AreaPOI");
    for name in ["ID", "Importance", "Icon", "FactionID"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s.add_field(SchemaField::new_array("Pos", FieldType::Float32, 3));
    for name in ["ContinentID", "Flags", "AreaID"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    for (name_col, flags_col) in [("Name", "NameFlags"), ("Description", "DescriptionFlags")] {
        s.add_field(SchemaField::new(name_col, FieldType::String)); // enUS (locale 0)
        s.add_field(SchemaField::new_array(
            format!("{name_col}OtherLocales"),
            FieldType::String,
            7,
        ));
        s.add_field(SchemaField::new(flags_col, FieldType::UInt32));
    }
    s.add_field(SchemaField::new("WorldStateID", FieldType::UInt32));
    s
}

/// Read `AreaPOI.dbc` off the patch chain into an [`AreaPoiCatalog`].
pub fn load_area_poi_catalog(chain: &mut Chain) -> Result<AreaPoiCatalog> {
    let bytes = chain.read_file(AREA_POI).context("reading AreaPOI.dbc")?;
    let rs = parse(&bytes, schema(), "AreaPOI")?;
    let mut rows = Vec::with_capacity(rs.records().len());
    let mut by_id = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let (Some(x), Some(y), Some(z)) = (f32_at(r, 4), f32_at(r, 5), f32_at(r, 6)) else {
            continue;
        };
        let poi = AreaPoi {
            importance: u32_at(r, 1).unwrap_or(0),
            icon: u32_at(r, 2).unwrap_or(0),
            faction_id: u32_at(r, 3).unwrap_or(0),
            pos: [x, y, z],
            continent_id: u32_at(r, 7).unwrap_or(0),
            flags: u32_at(r, 8).unwrap_or(0),
            area_id: u32_at(r, 9).unwrap_or(0),
            name: str_at(&rs, r, 10).unwrap_or_default(),
            description: str_at(&rs, r, 19).unwrap_or_default(),
            world_state_id: u32_at(r, 28).unwrap_or(0),
        };
        by_id.insert(id, rows.len());
        rows.push((id, poi));
    }
    Ok(AreaPoiCatalog { rows, by_id })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 5875 layout: readable names, positions in range, and Arathi Basin's Stables node.
    #[test]
    fn real_area_poi_layout_sanity() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_area_poi_catalog(&mut chain).expect("load AreaPOI");
        assert_eq!(cat.len(), 339, "all 339 rows load");

        let mut nonempty_names = 0;
        let mut worldstate_nonzero = 0;
        for (_, poi) in cat.rows() {
            if !poi.name.is_empty() {
                nonempty_names += 1;
            }
            assert!(
                poi.pos[0].abs() <= 17066.0 && poi.pos[1].abs() <= 17066.0,
                "world x/y within the map half-extent: {:?}",
                poi.pos
            );
            assert!(poi.continent_id < 10_000, "continentId is a small map id");
            if poi.world_state_id != 0 {
                worldstate_nonzero += 1;
            }
        }
        assert_eq!(nonempty_names, 339, "every row has a readable enUS name");
        assert!(
            worldstate_nonzero >= 100,
            "a meaningful share of rows carry a live WorldStateID: {worldstate_nonzero}"
        );

        let stables = cat.get(1613).expect("id 1613 (Stables)");
        assert_eq!(stables.name, "Stables");
        assert_eq!(stables.continent_id, 529, "Arathi Basin's map id");
        assert_eq!(stables.world_state_id, 1770);
        assert_eq!(stables.description, "In Conflict");
    }
}
