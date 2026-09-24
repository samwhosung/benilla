//! `AreaTable.dbc`, the area catalog. The world map walks an MCNK `areaId` up the parent chain to
//! its top-level zone (`SetMapToCurrentZone`) and labels it with the localized `AreaName`
//! ("Elwynn Forest"; `WorldMapArea` holds the art folder, "Elwynn").

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::chain::Chain;
use crate::dbc::{parse, str_at, u32_at};

const AREA_TABLE: &str = "DBFilesClient\\AreaTable.dbc";

/// One `AreaTable.dbc` row, the columns the map reads.
#[derive(Clone, Debug)]
pub struct AreaTableRow {
    /// `Map.dbc` id the area lives on.
    pub map_id: u32,
    /// The parent area's id; `0` means this row is itself a top-level zone.
    pub zone_id: u32,
    /// The exploration bit index into `PLAYER_EXPLORED_ZONES`, also the `WorldMapOverlay` join.
    pub explore_flag: u32,
    /// Bit `0x80` marks an FFA duel pit row (not its arena's), `0x1` snow; capitals carry `0x138`.
    pub flags: u32,
    /// `FactionGroup.dbc` mask of the owner: 2 Alliance, 4 Horde, 0 contested or a subzone row.
    pub faction_group_mask: u32,
    /// `ExplorationLevel` (`AreaTable+0x28`), read for its sign: a world-map landmark on a row
    /// `>= 0` stays hidden until the area is explored, `-1` exempts it (`0x4a6890`-`0x4a68f3`).
    pub exploration_level: i32,
    /// The localized display name ("Elwynn Forest").
    pub name: String,
}

/// `AreaTable.dbc` rows keyed by id.
pub struct AreaTableCatalog {
    by_id: HashMap<u32, AreaTableRow>,
}

impl AreaTableCatalog {
    /// A catalog over rows given directly, for tests that need a table of known shape.
    pub fn from_rows(rows: Vec<(u32, AreaTableRow)>) -> Self {
        Self {
            by_id: rows.into_iter().collect(),
        }
    }

    pub fn get(&self, id: u32) -> Option<&AreaTableRow> {
        self.by_id.get(&id)
    }

    /// The localized display name for `id`.
    pub fn name(&self, id: u32) -> Option<&str> {
        self.by_id.get(&id).map(|r| r.name.as_str())
    }

    /// The top-level zone above `area_id`, or itself; a chain deeper than eight is corrupt data.
    pub fn top_zone(&self, area_id: u32) -> Option<u32> {
        let mut id = area_id;
        for _ in 0..8 {
            let row = self.by_id.get(&id)?;
            if row.zone_id == 0 {
                return Some(id);
            }
            id = row.zone_id;
        }
        None
    }

    /// Whether a unit standing in `area_id` puffs visible breath (`0x67e9c0`): bit `0x1` of the
    /// leaf's flags when it sets bit `0x2` or has no parent row, else of its parent's. One hop,
    /// never a chain walk, and no weather or indoor input.
    pub fn is_cold(&self, area_id: u32) -> bool {
        let Some(leaf) = self.by_id.get(&area_id) else {
            return false;
        };
        let row = if leaf.flags & 0x2 != 0 {
            leaf
        } else {
            self.by_id.get(&leaf.zone_id).unwrap_or(leaf)
        };
        row.flags & 0x1 != 0
    }

    /// The id of the area named `name` (any case) for `/who`'s `z-` term; a top-level row wins.
    pub fn id_for_name(&self, name: &str) -> Option<u32> {
        let mut fallback = None;
        for (id, row) in &self.by_id {
            if !row.name.eq_ignore_ascii_case(name) {
                continue;
            }
            if row.zone_id == 0 {
                return Some(*id);
            }
            fallback = Some(*id);
        }
        fallback
    }

    /// Every row, unordered, for a scan by flag (the chat auto-join's `Flags & 0x200` city row).
    pub fn rows(&self) -> impl Iterator<Item = &AreaTableRow> {
        self.by_id.values()
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("AreaTable");
    for i in 0..25 {
        match i {
            11 => s.add_field(SchemaField::new("AreaName", FieldType::String)),
            _ => s.add_field(SchemaField::new(format!("c{i}"), FieldType::UInt32)),
        }
    }
    s
}

/// Read `AreaTable.dbc` off the patch chain into an [`AreaTableCatalog`].
pub fn load_area_table_catalog(chain: &mut Chain) -> Result<AreaTableCatalog> {
    let bytes = chain
        .read_file(AREA_TABLE)
        .context("reading AreaTable.dbc")?;
    let rs = parse(&bytes, schema(), "AreaTable")?;
    let mut by_id = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let (Some(id), Some(map_id), Some(zone_id), Some(explore_flag), Some(flags), Some(name)) = (
            u32_at(r, 0),
            u32_at(r, 1),
            u32_at(r, 2),
            u32_at(r, 3),
            u32_at(r, 4),
            str_at(&rs, r, 11),
        ) else {
            continue;
        };
        let faction_group_mask = u32_at(r, 20).unwrap_or(0);
        // Signed; an absent column reads as `-1`, no exploration gate.
        let exploration_level = u32_at(r, 10).map_or(-1, |v| v as i32);
        by_id.insert(
            id,
            AreaTableRow {
                map_id,
                zone_id,
                explore_flag,
                flags,
                faction_group_mask,
                exploration_level,
                name,
            },
        );
    }
    Ok(AreaTableCatalog { by_id })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_area_table_parent_chains_and_names() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_area_table_catalog(&mut chain).expect("load AreaTable");
        assert!(
            cat.len() > 1000,
            "5875 ships ~1800 areas, got {}",
            cat.len()
        );

        let northshire = cat.get(9).expect("Northshire Valley (id 9)");
        assert_eq!(northshire.zone_id, 12, "Northshire's parent is Elwynn");
        assert_eq!(northshire.name, "Northshire Valley");
        assert_eq!(cat.top_zone(9), Some(12));

        // Booty Bay (35) sits under Stranglethorn Vale (33), a top-level zone.
        assert_eq!(cat.top_zone(35), Some(33));

        assert_eq!(cat.top_zone(12), Some(12));
        assert_eq!(cat.name(12), Some("Elwynn Forest"));

        assert_eq!(cat.get(12).expect("Elwynn").explore_flag, 126);

        assert_eq!(cat.get(12).expect("Elwynn").faction_group_mask, 2);
        assert_eq!(cat.get(14).expect("Durotar").faction_group_mask, 4);
        assert_eq!(cat.get(33).expect("Stranglethorn").faction_group_mask, 0);
        // The FFA-pit bit is on the pit row, not the enclosing arena row.
        assert_ne!(cat.get(2177).expect("Battle Ring").flags & 0x80, 0);
        assert_eq!(cat.get(1741).expect("Gurubashi Arena").flags & 0x80, 0);
    }

    /// The sub-areas carry `0x40`, so only the one-hop inheritance makes them cold.
    #[test]
    fn cold_areas_inherit_one_hop_from_their_zone() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_area_table_catalog(&mut chain).expect("load AreaTable");

        // The four authored rows.
        for (id, label) in [
            (1, "Dun Morogh"),
            (618, "Winterspring"),
            (722, "Razorfen Downs"),
            (3456, "Naxxramas"),
        ] {
            assert_ne!(cat.get(id).expect(label).flags & 0x1, 0, "{label} authored");
            assert!(cat.is_cold(id), "{label} is cold");
        }

        for (id, label) in [
            (131, "Kharanos"),
            (132, "Coldridge Valley"),
            (136, "The Grizzled Den"),
            (2255, "Everlook"),
        ] {
            let row = cat.get(id).expect(label);
            assert_eq!(row.flags & 0x3, 0, "{label} authors neither bit itself");
            assert!(cat.is_cold(id), "{label} inherits its zone's cold");
        }

        // Gnomeregan the Dun Morogh sub-area (133) is cold; the dungeon row (721), its own
        // top-level zone, is not.
        assert!(cat.is_cold(133), "Gnomeregan the sub-area");
        for (id, label) in [
            (721, "Gnomeregan (dungeon)"),
            (12, "Elwynn Forest"),
            (1519, "Stormwind City"),
            (36, "Alterac Mountains"),
            (267, "Hillsbrad Foothills"),
        ] {
            assert!(!cat.is_cold(id), "{label} is not cold");
        }
        assert!(!cat.is_cold(0), "an unknown area is not cold");
    }
}
