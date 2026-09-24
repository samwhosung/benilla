//! `WorldMapContinent.dbc` — one row per continent (Azeroth, Kalimdor) framing the continent-level
//! map art: which ADT tile-grid span it covers, its placement offset/scale in the shared world-map
//! coordinate space, and the taxi-map's own bounding rect (decision 0203 phase 2's continent view).
//!
//! **Client-only: vmangos carries no struct for this table** (never loaded server-side). Layout
//! **VERIFIED against the 5875 binary** (2026-07-07): **2 × 13 × 52 B** —
//! `ID(0), MapID(1)`; fields 2-5 = ADT tile bounds (ints; the world-map builder `0x4a5d00`
//! converts them to each continent's normalized-UV sheet rect — the world-level click AABB;
//! `map_proj::continent_sheet_rect` transcribes the kernel); fields 6/7 = the per-axis
//! world-sheet offsets and field 8 = the scale, exactly the values `0x4a72b0`/`0x4a7360`
//! world-mode read at record `+0x18/+0x1c/+0x20` (5875: EK `{14.5, −7.0, 0.75}`, Kalimdor
//! `{−19.0, −0.32249799, 0.75}`); fields 9-12 = the taxi-map world rect, **unused** by all four
//! projection functions.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, u32_at};
use crate::Chain;

const WORLD_MAP_CONTINENT: &str = "DBFilesClient\\WorldMapContinent.dbc";

/// One `WorldMapContinent.dbc` row (all fields decomp-verified — see the module doc).
#[derive(Clone, Debug)]
pub struct WorldMapContinent {
    /// `Map.dbc` id (`0` = Azeroth, `1` = Kalimdor in 5875).
    pub map_id: u32,
    /// ADT tile-grid column bounds — the `0x4a5d00` sheet-rect kernel's inputs (see module doc).
    pub left_boundary: u32,
    pub right_boundary: u32,
    /// ADT tile-grid row bounds.
    pub top_boundary: u32,
    pub bottom_boundary: u32,
    /// The continent's world-sheet offsets — the `f` terms of the world-mode projections
    /// (`0x4a7360`: `u = offset_x/62.625 + 0.5 − wy·k·scale`), x → the u axis, y → v.
    pub offset_x: f32,
    pub offset_y: f32,
    /// The world-sheet scale the projections multiply by (`0.75` on both 5875 rows — there is
    /// no separate world-level zoom variable).
    pub scale: f32,
    /// The taxi-map's own bounding rect in world units — carried for the future taxi layer;
    /// unused by the map projections.
    pub taxi_min: (f32, f32),
    pub taxi_max: (f32, f32),
}

/// `WorldMapContinent.dbc` rows keyed by `MapID`.
pub struct WorldMapContinentCatalog {
    by_map_id: HashMap<u32, WorldMapContinent>,
}

impl WorldMapContinentCatalog {
    pub fn get(&self, map_id: u32) -> Option<&WorldMapContinent> {
        self.by_map_id.get(&map_id)
    }

    pub fn len(&self) -> usize {
        self.by_map_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_map_id.is_empty()
    }
}

/// 13 fields: `ID, MapID, LeftBoundary, RightBoundary, TopBoundary, BottomBoundary, OffsetX,
/// OffsetY, Scale, TaxiMinX, TaxiMinY, TaxiMaxX, TaxiMaxY` (see the module doc for confidence).
fn schema() -> Schema {
    let mut s = Schema::new("WorldMapContinent");
    for name in [
        "ID",
        "MapID",
        "LeftBoundary",
        "RightBoundary",
        "TopBoundary",
        "BottomBoundary",
    ] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    for name in [
        "OffsetX", "OffsetY", "Scale", "TaxiMinX", "TaxiMinY", "TaxiMaxX", "TaxiMaxY",
    ] {
        s.add_field(SchemaField::new(name, FieldType::Float32));
    }
    s
}

/// Read `WorldMapContinent.dbc` off the patch chain into a [`WorldMapContinentCatalog`].
pub fn load_world_map_continent_catalog(chain: &mut Chain) -> Result<WorldMapContinentCatalog> {
    let bytes = chain
        .read_file(WORLD_MAP_CONTINENT)
        .context("reading WorldMapContinent.dbc")?;
    let rs = parse(&bytes, schema(), "WorldMapContinent")?;
    let mut by_map_id = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(map_id) = u32_at(r, 1) else { continue };
        let (
            Some(left_boundary),
            Some(right_boundary),
            Some(top_boundary),
            Some(bottom_boundary),
            Some(offset_x),
            Some(offset_y),
            Some(scale),
            Some(taxi_min_x),
            Some(taxi_min_y),
            Some(taxi_max_x),
            Some(taxi_max_y),
        ) = (
            u32_at(r, 2),
            u32_at(r, 3),
            u32_at(r, 4),
            u32_at(r, 5),
            f32_at(r, 6),
            f32_at(r, 7),
            f32_at(r, 8),
            f32_at(r, 9),
            f32_at(r, 10),
            f32_at(r, 11),
            f32_at(r, 12),
        )
        else {
            continue;
        };
        by_map_id.insert(
            map_id,
            WorldMapContinent {
                map_id,
                left_boundary,
                right_boundary,
                top_boundary,
                bottom_boundary,
                offset_x,
                offset_y,
                scale,
                taxi_min: (taxi_min_x, taxi_min_y),
                taxi_max: (taxi_max_x, taxi_max_y),
            },
        );
    }
    Ok(WorldMapContinentCatalog { by_map_id })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real 5875 table: exactly the two verified rows (Azeroth mapId 0, Kalimdor mapId 1),
    /// byte-exact on every field. Skips without client data.
    #[test]
    fn real_world_map_continent_has_azeroth_and_kalimdor() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_world_map_continent_catalog(&mut chain).expect("load WorldMapContinent");
        assert_eq!(cat.len(), 2);

        let azeroth = cat.get(0).expect("Azeroth (mapId 0)");
        assert_eq!(
            (
                azeroth.left_boundary,
                azeroth.right_boundary,
                azeroth.top_boundary,
                azeroth.bottom_boundary
            ),
            (23, 47, 15, 61)
        );
        assert!((azeroth.offset_x - 14.5).abs() < 0.01);
        assert!((azeroth.offset_y - (-7.0)).abs() < 0.01);
        assert!((azeroth.scale - 0.75).abs() < 0.001);
        assert!((azeroth.taxi_min.0 - (-15980.0)).abs() < 0.1);
        assert!((azeroth.taxi_min.1 - (-11880.0)).abs() < 0.1);
        assert!((azeroth.taxi_max.0 - 5817.0).abs() < 0.1);
        assert!((azeroth.taxi_max.1 - 9917.0).abs() < 0.1);

        let kalimdor = cat.get(1).expect("Kalimdor (mapId 1)");
        assert_eq!(
            (
                kalimdor.left_boundary,
                kalimdor.right_boundary,
                kalimdor.top_boundary,
                kalimdor.bottom_boundary
            ),
            (23, 48, 9, 52)
        );
        assert!((kalimdor.scale - 0.75).abs() < 0.001);
    }
}
