//! `Interface\WorldMap\<AreaName>.zmp` (`0x845374`), named by the continent's `WorldMapArea` row:
//! a 128×128 row-major grid of `AreaTable.dbc` ids over the 64×64-tile world, two cells per tile
//! edge. The world map resolves the cell under the cursor to a zone through it (`0x4a6ec0`); the
//! loader (`0x4a5d00`) remaps cells to `WorldMapArea` ids.

use anyhow::{bail, Context, Result};

use crate::chain::Chain;

/// Cells per grid edge (half-ADT: 128 cells over 64 tiles).
pub const ZONE_MAP_EDGE: usize = 128;

/// Load `Interface\WorldMap\<area_name>.zmp` as raw `AreaTable.dbc` ids. A missing file is an
/// error here, where the client leaves its grid zeroed.
pub fn load_zone_map(chain: &mut Chain, area_name: &str) -> Result<Vec<u32>> {
    let path = format!("Interface\\WorldMap\\{area_name}.zmp");
    let bytes = chain
        .read_file(&path)
        .with_context(|| format!("reading {path}"))?;
    if bytes.len() != ZONE_MAP_EDGE * ZONE_MAP_EDGE * 4 {
        bail!(
            "{path}: expected {} bytes (128×128 u32), got {}",
            ZONE_MAP_EDGE * ZONE_MAP_EDGE * 4,
            bytes.len()
        );
    }
    Ok(bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cell 12735 is Goldshire: row from wx = -9450, column from wy = 60.
    #[test]
    fn real_azeroth_zone_map_goldshire_cell() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let grid = load_zone_map(&mut chain, "Azeroth").expect("load Azeroth.zmp");
        assert_eq!(grid.len(), 16384);
        assert_eq!(grid[12735], 12, "the Goldshire cell is Elwynn Forest");
        let nonzero = grid.iter().filter(|&&v| v != 0).count();
        assert!(
            (2000..6000).contains(&nonzero),
            "land cells in a plausible band, got {nonzero}"
        );
    }
}
