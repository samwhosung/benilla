//! `WorldMapArea.dbc` — the world-map projection basis: for each map-UI "area" (a continent as a
//! whole, or one zone/instance's own map), the art folder under `Interface\WorldMap\<name>\` and
//! the world-coordinate rect that folder's texture covers (decision 0203's `map_proj`, phase 2,
//! reads this rect to lerp between world space and normalized map UV space).
//!
//! Layout — VERIFIED against build 5875 (header + full 51-row decode, cross-checked against
//! vmangos's `WorldMapAreaEntry` (`src/game/Database/DBCStructure.h:737`), 2026-07-07): **51 × 8 ×
//! 32 B**: `ID(0), MapID(1), AreaID(2, 0 = the continent-wide row), AreaName(3, string — the
//! `Interface\WorldMap\<name>\` folder), LocLeft(4), LocRight(5), LocTop(6), LocBottom(7)` (4×
//! `f32`). vmangos's own struct skips `ID` (commented out, unused server-side) and names the loc
//! quad `y1/y2/x1/x2`; every other field lines up 1:1. Byte-exact on the two rows that matter for
//! phase 2's continent basis: id 14 → `(mapId 0, areaId 0, "Azeroth", [16000.0, -19199.9, 7466.6,
//! -16000.0])`, id 13 → `(mapId 1, areaId 0, "Kalimdor", ...)`; a zone row, id 4 → `(mapId 1,
//! areaId 14, "Durotar", [-1962.5, -7250.0, 1808.3, -1716.7])`, whose `AreaName` is confirmed (this
//! session) to be a real `Interface\WorldMap\Durotar\Durotar1.blp` art folder in the chain.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, str_at, u32_at};
use crate::Chain;

const WORLD_MAP_AREA: &str = "DBFilesClient\\WorldMapArea.dbc";

/// One `WorldMapArea.dbc` row: a map-UI "area" (continent or zone) and the world rect its
/// `Interface\WorldMap\<name>\` art covers.
#[derive(Clone, Debug)]
pub struct WorldMapArea {
    /// `Map.dbc` id this row belongs to.
    pub map_id: u32,
    /// `AreaTable.dbc` id for a zone row; `0` for the continent-wide row (Azeroth/Kalimdor/…).
    pub area_id: u32,
    /// The `Interface\WorldMap\<name>\` art folder (also the client-visible internal name).
    pub name: String,
    /// World-coordinate rect this area's map art covers (vmangos: `y1/y2/x1/x2` — WoW's world X is
    /// "top/bottom", world Y is "left/right"; see the module doc for the verified sample values).
    pub loc_left: f32,
    pub loc_right: f32,
    pub loc_top: f32,
    pub loc_bottom: f32,
}

/// `WorldMapArea.dbc` rows keyed by `ID` — the id `WorldMapOverlay.worldMapAreaId` joins against.
/// File order is preserved ([`WorldMapAreaCatalog::iter`]): the client's continent index IS the
/// on-disk order of the areaId==0 rows (Kalimdor before Azeroth in 5875; the builder `0x4a5d00`
/// walks rows in file order).
pub struct WorldMapAreaCatalog {
    by_id: HashMap<u32, WorldMapArea>,
    /// Row ids in on-disk record order.
    file_order: Vec<u32>,
}

impl WorldMapAreaCatalog {
    /// The row for `id` (the `WorldMapArea.dbc` primary key), or `None`.
    pub fn get(&self, id: u32) -> Option<&WorldMapArea> {
        self.by_id.get(&id)
    }

    /// The continent-wide row (`area_id == 0`) for `map_id` — the "world" projection basis
    /// (`map_proj`'s continent mode). Exactly one such row per continent in 5875
    /// (Azeroth id 14, Kalimdor id 13).
    pub fn continent(&self, map_id: u32) -> Option<(u32, &WorldMapArea)> {
        self.by_id
            .iter()
            .find(|(_, a)| a.map_id == map_id && a.area_id == 0)
            .map(|(&id, a)| (id, a))
    }

    /// All rows as `(id, row)`, in **file order** — the order the client's continent index is
    /// built in (see the struct doc). Zone consumers re-sort by display name anyway.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &WorldMapArea)> {
        self.file_order
            .iter()
            .filter_map(|id| self.by_id.get(id).map(|a| (*id, a)))
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

/// 8 fields: `ID, MapID, AreaID, AreaName, LocLeft, LocRight, LocTop, LocBottom`.
fn schema() -> Schema {
    let mut s = Schema::new("WorldMapArea");
    for name in ["ID", "MapID", "AreaID"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s.add_field(SchemaField::new("AreaName", FieldType::String));
    for name in ["LocLeft", "LocRight", "LocTop", "LocBottom"] {
        s.add_field(SchemaField::new(name, FieldType::Float32));
    }
    s
}

/// Read `WorldMapArea.dbc` off the patch chain into a [`WorldMapAreaCatalog`].
pub fn load_world_map_area_catalog(chain: &mut Chain) -> Result<WorldMapAreaCatalog> {
    let bytes = chain
        .read_file(WORLD_MAP_AREA)
        .context("reading WorldMapArea.dbc")?;
    let rs = parse(&bytes, schema(), "WorldMapArea")?;
    let mut by_id = HashMap::with_capacity(rs.records().len());
    let mut file_order = Vec::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let (
            Some(map_id),
            Some(area_id),
            Some(name),
            Some(loc_left),
            Some(loc_right),
            Some(loc_top),
            Some(loc_bottom),
        ) = (
            u32_at(r, 1),
            u32_at(r, 2),
            str_at(&rs, r, 3),
            f32_at(r, 4),
            f32_at(r, 5),
            f32_at(r, 6),
            f32_at(r, 7),
        )
        else {
            continue;
        };
        file_order.push(id);
        by_id.insert(
            id,
            WorldMapArea {
                map_id,
                area_id,
                name,
                loc_left,
                loc_right,
                loc_top,
                loc_bottom,
            },
        );
    }
    Ok(WorldMapAreaCatalog { by_id, file_order })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real 5875 table: the continent rows (Azeroth/Kalimdor) plus Durotar's verified rect,
    /// and the zone row's `name` really is a `Interface\WorldMap\<name>\` art folder in the chain
    /// (proving `AreaName` is the art folder, not just a label). Skips without client data.
    #[test]
    fn real_world_map_area_has_continents_and_durotar_and_art_folder() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_world_map_area_catalog(&mut chain).expect("load WorldMapArea");
        assert_eq!(cat.len(), 51, "all 51 rows load");

        let azeroth = cat.get(14).expect("Azeroth continent row (id 14)");
        assert_eq!((azeroth.map_id, azeroth.area_id), (0, 0));
        assert_eq!(azeroth.name, "Azeroth");

        let kalimdor = cat.get(13).expect("Kalimdor continent row (id 13)");
        assert_eq!((kalimdor.map_id, kalimdor.area_id), (1, 0));
        assert_eq!(kalimdor.name, "Kalimdor");

        // `continent()` resolves the same two rows by map_id alone.
        let (azeroth_id, _) = cat.continent(0).expect("Azeroth via continent(0)");
        assert_eq!(azeroth_id, 14);
        let (kalimdor_id, _) = cat.continent(1).expect("Kalimdor via continent(1)");
        assert_eq!(kalimdor_id, 13);

        let durotar = cat.get(4).expect("Durotar row (id 4)");
        assert_eq!((durotar.map_id, durotar.area_id), (1, 14));
        assert_eq!(durotar.name, "Durotar");
        assert!((durotar.loc_left - (-1962.5)).abs() < 0.1);
        assert!((durotar.loc_right - (-7250.0)).abs() < 0.1);
        assert!((durotar.loc_top - 1808.3).abs() < 0.1);
        assert!((durotar.loc_bottom - (-1716.7)).abs() < 0.1);

        // The zone row's name really is the `Interface\WorldMap\<name>\` art folder.
        let art = format!("Interface\\WorldMap\\{0}\\{0}1.blp", durotar.name);
        assert!(
            chain.contains(&art),
            "Durotar's WorldMapArea name resolves to its own art folder: {art}"
        );
    }
}
