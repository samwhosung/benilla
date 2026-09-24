//! `AreaTable.dbc` — the area catalog: id → (map, parent zone, explore bit, name). The world map
//! (decision 0203 phase 2) resolves the player's MCNK `areaId` to its **top-level zone** through
//! the parent chain here (`SetMapToCurrentZone`), and labels zones with the localized `AreaName`
//! (the WorldMapArea string is the art *folder* — "Elwynn" — while the display name lives here —
//! "Elwynn Forest"). Phase 3's exploration overlays key off [`AreaTableRow::explore_flag`].
//!
//! Layout — VERIFIED against build 5875 (2026-07-07, real-row decode: Northshire Valley id 9 →
//! zone 12 = Elwynn Forest; Booty Bay id 35 → zone 33 = Stranglethorn Vale; cross-checked against
//! mangoszero's `AreaTableEntry`): **25 × u32 cols**: `ID(0), MapID(1), ZoneID(2 — the parent
//! area, 0 = this row IS a top-level zone), ExploreFlag(3), Flags(4), … ExplorationLevel(10, the
//! binary's `+0x28`), AreaName(11)` + loc block, `FactionGroupMask(20)`. Sibling readers of the
//! same file: `quest_headers.rs` (id → name only), `area_sound.rs` (the audio columns).
//!
//! The PvP columns (zone-splash arc, decision 0287) — census of the real 5875 table:
//! `FactionGroupMask` is 2/4/0 (Alliance/Horde/neither) on zone rows and ~always 0 on subzone
//! rows; `Flags` bit `0x80` marks exactly the three FFA duel pits (Battle Ring 2177, The Rumble
//! Cage 2857, The Maul 3217 + its UNUSED twin) — *not* the enclosing "Gurubashi Arena" row.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::chain::Chain;
use crate::dbc::{parse, str_at, u32_at};

const AREA_TABLE: &str = "DBFilesClient\\AreaTable.dbc";

/// One `AreaTable.dbc` row (the columns the map arc reads).
#[derive(Clone, Debug)]
pub struct AreaTableRow {
    /// `Map.dbc` id the area lives on.
    pub map_id: u32,
    /// The parent area's id — `0` means this row is itself a top-level zone.
    pub zone_id: u32,
    /// The exploration bit index (`PLAYER_EXPLORED_ZONES` bitset / `WorldMapOverlay` join,
    /// phase 3).
    pub explore_flag: u32,
    /// Area flags (col 4). Bit `0x80` = FFA duel pit ("PvP Area" / `isArena`, decision 0287);
    /// bit `0x1` = snow; capitals carry `0x138`.
    pub flags: u32,
    /// `FactionGroup.dbc` mask of the owning side (col 20): 2 = Alliance, 4 = Horde, 0 = neither
    /// (contested — or a subzone row deferring to its zone).
    pub faction_group_mask: u32,
    /// `ExplorationLevel` (col 10, the binary's `AreaTable+0x28`) — **signed**, and read only as
    /// `>= 0` vs `< 0`. It is the world-map landmark builder's exploration gate: a landmark whose
    /// `AreaID` lands on a row with `>= 0` here stays hidden until the player has explored that
    /// area ([`Self::explore_flag`]'s bit); `-1` exempts it (`0x4a6890`–`0x4a68f3`). The 5875 table
    /// is `-1` on exactly one row and `>= 0` on the other 1080, so in practice the gate applies
    /// wherever an `AreaID` resolves at all.
    pub exploration_level: i32,
    /// The localized display name ("Elwynn Forest").
    pub name: String,
}

/// `AreaTable.dbc` rows keyed by id.
pub struct AreaTableCatalog {
    by_id: HashMap<u32, AreaTableRow>,
}

impl AreaTableCatalog {
    /// A catalog over rows given directly, rather than read off the chain — for a caller that
    /// needs a table of exactly known shape (the map-arc gate tests build two-row ones, so that
    /// what a gate does with `ExplorationLevel -1` is asserted rather than hoped for).
    pub fn from_rows(rows: Vec<(u32, AreaTableRow)>) -> Self {
        Self {
            by_id: rows.into_iter().collect(),
        }
    }

    pub fn get(&self, id: u32) -> Option<&AreaTableRow> {
        self.by_id.get(&id)
    }

    /// The display name for `id`, or `None`.
    pub fn name(&self, id: u32) -> Option<&str> {
        self.by_id.get(&id).map(|r| r.name.as_str())
    }

    /// The top-level zone containing `area_id`: walk the `zone_id` parent chain until a row whose
    /// parent is 0 (possibly `area_id` itself). `None` for an unknown id. Depth-guarded — the
    /// 5875 chains are 1–2 deep, anything deeper is data corruption, not geography.
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

    /// Is `area_id` **cold** — does a unit standing here puff visible breath?
    ///
    /// The client's `0x67e9c0`, byte-exact:
    ///
    /// ```text
    /// cold ⟺ ((leaf.Flags & 0x2) || no valid parent ? leaf.Flags : parent.Flags) & 0x1
    /// ```
    ///
    /// **One hop, never a chain walk** ([`Self::top_zone`] is a different question with a
    /// different answer). Bit `0x1` is authored on exactly four leaf rows in 5875 — Dun Morogh,
    /// Winterspring, Razorfen Downs, Naxxramas — and the single-hop inheritance is what spreads it
    /// to the 45 areas that are actually cold: every sub-area of Dun Morogh and Winterspring
    /// carries `0x40` (bit `0x2` clear) and therefore reads its zone's flags instead. Bit `0x2` is
    /// the opt-out that makes a sub-area answer for itself.
    ///
    /// This is **independent of the weather**: snowfall and breath vapour share no input, and
    /// there is no indoor suppression — standing in the Thunderbrew Distillery is as cold as the
    /// road outside, because indoor-ness only changes *which row* resolves, not this test.
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

    /// The id of the zone **named** `name`, case-insensitively — the reverse of [`Self::name`],
    /// for the one caller that has a name and needs the id: `/who`'s `z-"Elwynn Forest"` term,
    /// which goes on the wire as a zone id (decision 0668).
    ///
    /// Names are not unique across the table (a subzone can share its zone's name, instance rows
    /// repeat), so a **top-level** row (`zone_id == 0`) wins over any other match — that is what
    /// "zone" means to the caller. Linear, which is fine at one call per query.
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

    /// Every row, unordered — for the rare query that is a whole-table **scan by flag** rather
    /// than a lookup by id. The chat auto-join's city-word row (`Flags & 0x200`) is the first such
    /// consumer, and it is a scan in the client too.
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

/// 25 u32-wide columns; only
/// `ID/MapID/ZoneID/ExploreFlag/Flags/ExplorationLevel/AreaName/FactionGroupMask` are read (see
/// module doc).
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
        // Signed: `-1` is the "no exploration requirement" row. Absent reads as `-1` for the
        // same reason — an unreadable column must not invent a gate.
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

    /// The real 5875 table: the verified parent chains (Northshire → Elwynn, Booty Bay →
    /// Stranglethorn), a top-level zone resolving to itself, and the display-vs-art-folder name
    /// split. Skips without client data.
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

        // A leaf two hops down: Booty Bay → Stranglethorn Vale (top-level).
        assert_eq!(cat.top_zone(35), Some(33));

        // A top-level zone resolves to itself; its display name differs from the art folder.
        assert_eq!(cat.top_zone(12), Some(12));
        assert_eq!(cat.name(12), Some("Elwynn Forest"));

        // The phase-3 explore bit is populated (Elwynn's is 126 in 5875).
        assert_eq!(cat.get(12).expect("Elwynn").explore_flag, 126);

        // The PvP columns (decision 0287): ownership masks on zone rows…
        assert_eq!(cat.get(12).expect("Elwynn").faction_group_mask, 2);
        assert_eq!(cat.get(14).expect("Durotar").faction_group_mask, 4);
        assert_eq!(cat.get(33).expect("Stranglethorn").faction_group_mask, 0);
        // …and the FFA-pit bit on the leaf pit rows, NOT the enclosing arena row.
        assert_ne!(cat.get(2177).expect("Battle Ring").flags & 0x80, 0);
        assert_eq!(cat.get(1741).expect("Gurubashi Arena").flags & 0x80, 0);
    }

    /// The cold-breath predicate on the shipped table (`0x67e9c0`): bit `0x1` is authored on
    /// exactly four rows, and the ONE-HOP parent inheritance is what makes the sub-areas cold.
    /// This is the test that would have caught reading the leaf alone — every place a player
    /// actually stands in Dun Morogh is a sub-area whose own flags are `0x40`.
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

        // The sub-areas: own flags `0x40` (bit 0x1 clear, bit 0x2 clear) → they read the zone's.
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

        // Negative controls, including the sharp one: Gnomeregan the Dun Morogh sub-area (133) is
        // cold; Gnomeregan the dungeon row (721), which is its own top-level zone, is not.
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
