//! `Interface\WorldMap\<Continent>.zmp` — the continent area bitmap: a 128×128 grid of
//! `AreaTable.dbc` row ids (16384 little-endian u32, row-major, exactly 65536 bytes) covering the
//! whole 64×64-ADT world at half-ADT granularity. The client's world-map hover/click resolves a
//! cursor cell to a zone through it (`0x4a6ec0`; the loader `0x4a5d00` remaps cells to
//! WorldMapArea ids at load — benilla's remap lives with the catalog build, `ui_world_map`).
//!
//! Mechanism VERIFIED against the 5875 binary (2026-07-07): path format string @0x845374
//! (`Interface\WorldMap\%s.zmp`, `%s` = the continent WorldMapArea row's AreaName), 0x10000 bytes
//! read raw into the record. Corroborated against the shipped file this session: Azeroth.zmp's
//! cell at the recorded index law for Goldshire's world position holds 12 = Elwynn Forest.
//! 5875 ships `Azeroth.zmp` + `Kalimdor.zmp` (and a dead `Expansion01.zmp`).

use anyhow::{bail, Context, Result};

use crate::chain::Chain;

/// Cells per grid edge (half-ADT: 128 cells over 64 tiles).
pub const ZONE_MAP_EDGE: usize = 128;

/// Load `Interface\WorldMap\<area_name>.zmp` — 16384 raw `AreaTable.dbc` ids, row-major.
/// A continent without a shipped bitmap is an error (the client leaves its grid zeroed; callers
/// decide whether that's tolerable).
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

    /// The real Azeroth bitmap: the Goldshire cell (index by the byte law: col from wy=60, row
    /// from wx=−9450 ⇒ cell 12735) holds Elwynn Forest's AreaTable id — the end-to-end pin of
    /// path, layout, and index law against shipped data. Skips without client data.
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
