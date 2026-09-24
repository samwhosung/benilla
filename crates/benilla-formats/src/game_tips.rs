//! `GameTips.dbc` — the loading-screen "tip of the day".
//!
//! The 74 one-line hints the reference draws over a loading screen, each already carrying its own
//! `|cffffd100Tip:|r` prefix and a trailing `\r\n` or two in the data. Nothing in FrameXML reads
//! them — `showGameTips`' only 1.12 mention there is
//! `UIOptionsFrameCheckButtons["SHOW_TIPOFTHEDAY_TEXT"] = { index = 44, cvar = "showGameTips" }`
//! and the tooltip string "Uncheck this to hide the tip of the day in the load screens", so the
//! whole feature is engine-side.
//!
//! **The table is the reference's own growable array** at `[0xc0dcd0]` (data) / `[0xc0dcd4]`
//! (count), stride `0x28` = this record size, filled by the DBC loader `0x545f90` from the run of
//! per-table loaders at `0x5404bb`. `CGlueMgr::EnterWorld` (`0x46b500`) walks the count at
//! `0x46b684` and stores the chosen slot back into the `gameTip` CVar.
//!
//! Layout — the real 5875 file (10883 bytes, **74** records × 10 × u32, 7903-byte string block):
//! `ID(0), Text[8](1..8), TextMask(9)` — the ordinary 1.12 localized-string shape, and only the
//! first locale slot is populated in this chain. **Record order is not id order** (the file opens
//! on id 396 and closes on 412, ids 201..412 with no duplicates), which is why the index the
//! reference persists is a *record* index into the file as loaded, never an id.

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::chain::Chain;
use crate::dbc::{parse, str_at};

const GAME_TIPS: &str = "DBFilesClient\\GameTips.dbc";

/// The tips, in the file's own record order — the order the reference's array holds them in, and
/// so the order its persisted index counts through.
#[derive(Clone, Debug, Default)]
pub struct GameTipsCatalog {
    tips: Vec<String>,
}

impl GameTipsCatalog {
    /// A catalog over rows given directly — for a test that must run without an install.
    pub fn from_tips(tips: Vec<String>) -> Self {
        GameTipsCatalog { tips }
    }

    /// The tip at a **record index**, or `None` when the index is past the table (an index read
    /// back from a config file written by a different chain).
    pub fn get(&self, index: usize) -> Option<&str> {
        self.tips.get(index).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.tips.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tips.is_empty()
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("GameTips");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new_array("Text", FieldType::String, 8));
    s.add_field(SchemaField::new("TextMask", FieldType::UInt32));
    s.set_key_field("ID");
    s
}

pub fn load_game_tips(chain: &mut Chain) -> Result<GameTipsCatalog> {
    let bytes = chain.read_file(GAME_TIPS).context("reading GameTips.dbc")?;
    let rs = parse(&bytes, schema(), "GameTips")?;
    let mut tips = Vec::with_capacity(rs.records().len());
    for r in rs.records() {
        // A row whose localized slot is empty is dropped rather than kept as a blank: the
        // reference's array is what the index counts through, and a blank slot would be a
        // loading screen with no tip on it for no reason a player could act on.
        match str_at(&rs, r, 1) {
            Some(text) if !text.trim().is_empty() => tips.push(text),
            _ => {}
        }
    }
    Ok(GameTipsCatalog { tips })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped table: 74 tips, in file order, each already carrying the reference's own
    /// colour prefix. Skips without client data.
    #[test]
    fn the_real_table_is_seventy_four_tips_in_file_order() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_game_tips(&mut chain).expect("load GameTips");
        assert_eq!(cat.len(), 74, "the 5875 enGB chain ships 74 tips");
        assert!(
            cat.get(0).unwrap().starts_with("|cffffd100"),
            "the colour prefix is authored INTO the data, not added by the renderer: {:?}",
            cat.get(0)
        );
        assert!(
            cat.get(0).unwrap().contains("mini-map"),
            "record order, not id order — record 0 is id 396: {:?}",
            cat.get(0)
        );
        assert!(
            cat.get(74).is_none(),
            "one past the end is None, not a panic"
        );
    }
}
