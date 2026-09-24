//! `textures\Minimap\md5translate.trs`, the minimap tile index: each authored tile name to the
//! content-hashed `.blp` stored under `textures\Minimap\`, tiles with the same art sharing a hash.
//! CRLF text of `dir: <Dir>` headers and tab-separated `<Dir>\<file>.blp\t<hash>.blp` lines; every
//! line repeats its full directory, so the headers are skipped. ADT tiles
//! (`<MapDir>\map<X>_<Y>.blp`) and WMO interior tiles (`WMO\...\<name>_<group>_<X>_<Y>.blp`)
//! share the table.

use std::collections::HashMap;

use anyhow::{Context, Result};

use crate::Chain;

const MD5_TRANSLATE: &str = "textures\\Minimap\\md5translate.trs";

/// The parsed `md5translate.trs`: every left-hand path (lowercased) to its hashed `.blp` filename.
pub struct MinimapTranslate {
    entries: HashMap<String, String>,
}

impl MinimapTranslate {
    /// The hashed filename, under `textures\Minimap\`, of `map_dir`'s ADT tile `(x, y)` in any
    /// case; `None` where no art was authored, as over open ocean.
    pub fn tile(&self, map_dir: &str, x: u32, y: u32) -> Option<&str> {
        let key = format!("{map_dir}\\map{x}_{y}.blp").to_ascii_lowercase();
        self.entries.get(&key).map(String::as_str)
    }

    /// The hashed filename for any tile path (`\`-separated, any case), such as the WMO interior
    /// tile `wmo\KhazModan\Cities\Ironforge\ironforge_001_00_00.blp`.
    pub fn get(&self, logical_path: &str) -> Option<&str> {
        self.entries
            .get(&logical_path.to_ascii_lowercase())
            .map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Parse the text body, CRLF or LF, skipping the `dir:` headers.
fn parse_trs(text: &str) -> HashMap<String, String> {
    let mut entries = HashMap::new();
    for raw in text.split('\n') {
        let line = raw.trim_end_matches('\r').trim();
        if line.is_empty() || line.starts_with("dir:") {
            continue;
        }
        let Some((left, right)) = line.split_once('\t') else {
            continue;
        };
        entries.insert(left.to_ascii_lowercase(), right.to_string());
    }
    entries
}

/// Read `md5translate.trs` off the patch chain into a [`MinimapTranslate`].
pub fn load_minimap_translate(chain: &mut Chain) -> Result<MinimapTranslate> {
    let bytes = chain
        .read_file(MD5_TRANSLATE)
        .context("reading md5translate.trs")?;
    let text = String::from_utf8_lossy(&bytes);
    Ok(MinimapTranslate {
        entries: parse_trs(&text),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dir_headers_and_tab_separated_rows_case_insensitively() {
        let text = "dir: AhnQiraj\r\n\
             AhnQiraj\\map27_46.blp\t1fcd95d6d410e7557d6b62081c5e87b5.blp\r\n\
             dir: Azeroth\r\n\
             Azeroth\\map32_48.blp\tea283abc0bf9637c3fad5e840a65b38b.blp\r\n";
        let entries = parse_trs(text);
        assert_eq!(entries.len(), 2);
        let cat = MinimapTranslate { entries };
        assert_eq!(
            cat.tile("Azeroth", 32, 48),
            Some("ea283abc0bf9637c3fad5e840a65b38b.blp")
        );
        assert_eq!(
            cat.tile("azeroth", 32, 48),
            Some("ea283abc0bf9637c3fad5e840a65b38b.blp"),
            "map_dir is case-insensitive (MPQ path convention)"
        );
        assert_eq!(cat.tile("Azeroth", 99, 99), None);
    }

    /// The 5875 file: an Azeroth tile resolves to a hash that reads as a 256×256 BLP.
    #[test]
    fn real_md5translate_resolves_azeroth_tile_and_hash_is_readable() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_minimap_translate(&mut chain).expect("load md5translate.trs");
        assert_eq!(cat.len(), 8401, "all 8401 data rows parse to unique keys");

        let hash = cat
            .tile("Azeroth", 32, 48)
            .expect("Azeroth\\map32_48 resolves");
        assert_eq!(hash, "ea283abc0bf9637c3fad5e840a65b38b.blp");

        let tile_path = format!("textures\\Minimap\\{hash}");
        let (w, h, _rgba) =
            crate::read_texture_rgba(&mut chain, &tile_path).expect("hashed tile decodes as BLP");
        assert_eq!((w, h), (256, 256));

        // The interior key the minimap builds: the `.wmo` path without `World\` or extension,
        // then `_<group>_<X>_<Y>.blp`.
        assert!(
            cat.get("wmo\\KhazModan\\Cities\\Ironforge\\ironforge_001_00_00.blp")
                .is_some(),
            "the Ironforge group-1 interior tile resolves"
        );
    }
}
