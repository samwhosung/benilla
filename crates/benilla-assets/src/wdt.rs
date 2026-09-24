//! WDT → map tile-index asset loader.
//!
//! Decodes a map's tiny `.wdt` into a [`WdtIndex`] — the 64×64 `MAIN` tile-existence grid the
//! terrain streamer consults before requesting any `.adt`: open ocean authors no
//! tiles, and probing them wholesale spammed asset-server NotFound errors on every boat crossing.
//! It also carries the map's **global WMO** where there is one: on the 20 shipped
//! maps that author no terrain at all, that single placement is the entire world.
//! The parse itself is `benilla-wdt`'s [`WdtReader`] (oracle-tested in the 0021 migration); this
//! module only wraps it in the Bevy asset machinery.

use benilla_formats::{GlobalWmo, WdtFile, WdtReader, WowVersion};
use bevy::asset::io::Reader;
use bevy::asset::{Asset, AssetLoader, LoadContext};
use bevy::reflect::TypePath;

/// A map's parsed WDT: which of the 64×64 ADT tiles exist.
#[derive(Asset, TypePath)]
pub struct WdtIndex(WdtFile);

impl WdtIndex {
    /// Whether tile `(tile_x, tile_y)` has an `.adt` (`MAIN` flag bit 0) — the same index order as
    /// the `Map_<tile_x>_<tile_y>.adt` file names and [`benilla_formats::world_to_tile`].
    /// Out-of-range ⇒ `false`.
    pub fn has_tile(&self, tile_x: u32, tile_y: u32) -> bool {
        self.0
            .get_tile(tile_x as usize, tile_y as usize)
            .is_some_and(|t| t.has_adt)
    }

    /// The map's single global building, on a map with no terrain (`MPHD` bit 0) — see
    /// [`GlobalWmo`]. `None` on an ADT map.
    pub fn global_wmo(&self) -> Option<&GlobalWmo> {
        self.0.global_wmo()
    }
}

/// Bevy [`AssetLoader`] decoding a vanilla `*.wdt` → [`WdtIndex`].
#[derive(Default, TypePath)]
pub struct WdtIndexLoader;

impl AssetLoader for WdtIndexLoader {
    type Asset = WdtIndex;
    type Settings = ();
    type Error = std::io::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        _ctx: &mut LoadContext<'_>,
    ) -> Result<WdtIndex, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let wdt = WdtReader::new(std::io::Cursor::new(bytes), WowVersion::Classic).read()?;
        Ok(WdtIndex(wdt))
    }

    fn extensions(&self) -> &[&str] {
        &["wdt"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal synthetic WDT: MVER + a MAIN grid with exactly two tiles flagged present.
    /// Pins the parse AND the index order ([`WdtIndex::has_tile`] is `(tile_x, tile_y)`, the
    /// on-disk grid is row-major `y * 64 + x`).
    #[test]
    fn parses_main_grid_in_tile_index_order() {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"REVM"); // magics are reversed on disk
        buf.extend_from_slice(&4u32.to_le_bytes());
        buf.extend_from_slice(&18u32.to_le_bytes());
        buf.extend_from_slice(b"NIAM");
        buf.extend_from_slice(&(64u32 * 64 * 8).to_le_bytes());
        let mut grid = vec![0u8; 64 * 64 * 8];
        for (x, y) in [(3usize, 5usize), (63, 0)] {
            grid[(y * 64 + x) * 8] = 1; // flags bit 0 = has_adt
        }
        buf.extend_from_slice(&grid);

        let wdt = WdtReader::new(std::io::Cursor::new(buf), WowVersion::Classic)
            .read()
            .expect("synthetic WDT parses");
        let index = WdtIndex(wdt);
        assert!(index.has_tile(3, 5));
        assert!(index.has_tile(63, 0));
        assert!(
            !index.has_tile(5, 3),
            "index order is (tile_x, tile_y), not swapped"
        );
        assert!(!index.has_tile(0, 0));
        assert!(
            !index.has_tile(64, 64),
            "out of range is absent, not a panic"
        );
    }
}
