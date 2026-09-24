//! Quest-log tag names: the quest template's `Type` → the suffix after a quest-log title.
//! `QuestInfo.dbc` (`ID`, the `Name` loc-string) holds the sparse ids 1 Elite (not the later
//! "Group"), 21 Life, 41 PvP, 62 Raid, 81 Dungeon, 82 World Event and 83 Legendary; `Type` 0, most
//! quests, has no tag. `GetQuestLogTitle` (`0x4df930`) reads it through `0x4df2a0`: the cached
//! template's `+0x10` (the 5th `u32` of `SMSG_QUEST_QUERY_RESPONSE`), bounds-checked against the
//! max id (`0xc0d9d0`), indexes `0xc0d9cc` directly, and a missing row reaches Lua as `nil`. The
//! full table ships in `patch-2.MPQ`; `dbc.MPQ` has only PvP, Life, Elite and Raid. `Type`'s one
//! other reader, the `PLAYER_QUEST_LOG` watcher (`0x5dde6b`, `0x5ddf02`), fires tutorial 40 when an
//! Elite quest is accepted.

use std::collections::HashMap;

use anyhow::Result;

use crate::chain::Chain;
use crate::dbc::load_id_name_table;

/// `Type` → tag name.
#[derive(Debug, Default)]
pub struct QuestTagNames(HashMap<u32, String>);

impl QuestTagNames {
    /// The tag for a quest `Type`. `None` must reach Lua as `nil`, never `""`: the reference's
    /// quest-log row tests the tag's presence.
    pub fn resolve(&self, quest_type: u32) -> Option<&str> {
        self.0.get(&quest_type).map(String::as_str)
    }

    /// How many tags the table names (7 in 5875).
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Load the tag names through the patch chain.
pub fn load_quest_tag_names(chain: &mut Chain) -> Result<QuestTagNames> {
    Ok(QuestTagNames(load_id_name_table(
        chain,
        "DBFilesClient\\QuestInfo.dbc",
        1,
        10,
        "QuestInfo",
    )?))
}
