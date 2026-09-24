//! Quest-log **tag names**: the quest template's `Type` → the parenthesised suffix a quest-log row
//! carries after its title — `(Elite)`, `(Dungeon)`, `(Raid)`, `(PvP)`.
//!
//! `QuestInfo.dbc` on 5875 (read here through the client's own patch chain — it ships in
//! `patch-2.MPQ`) is **7 records × 10 u32 fields, record size 40**: `ID(0)`, `Name(1)`, the 7
//! remaining loc-block slots, and the loc flag mask at col 9 — the same `ID … Name … loc` shape
//! `QuestSort.dbc` has, so it reads through the same [`crate::dbc::load_id_name_table`].
//!
//! **The whole table**, in file order — these seven ids are the entire tag vocabulary a 1.12
//! client can render, and they are SPARSE, so the lookup is by id and never by row index:
//!
//! | ID | Name |
//! |----|------|
//! | 1  | `Elite` |
//! | 21 | `Life` |
//! | 41 | `PvP` |
//! | 62 | `Raid` |
//! | 81 | `Dungeon` |
//! | 82 | `World Event` |
//! | 83 | `Legendary` |
//!
//! Note `1 = "Elite"`, not the later expansions' `"Group"` — a 1.12 group quest reads `(Elite)`.
//!
//! The key is [`benilla_protocol::messages::QuestTemplate::quest_type`], the 5th `u32` of
//! `SMSG_QUEST_QUERY_RESPONSE` (vmangos writes `pQuest->GetType()` there, and its
//! `quest_template.Type` carries exactly these sparse ids). A quest with `Type == 0` — the great
//! majority — names no row and takes no tag.
//!
//! All of that is **read off the binary**:
//! `0x4df930` reads the cached template's `+0x10`, bounds-checks it against the table's maxId
//! (`0xc0d9d0`) and indexes `0xc0d9cc` with no arithmetic on the id; type `0` passes the bounds
//! and dies on the zero-filled slot, which reaches Lua as `nil` because `lua_pushstring` tail-jumps
//! to `lua_pushnil` on NULL. **Read this path through the patch chain and nowhere else:** the base
//! `dbc.MPQ` copy has only four rows (PvP, Life, Elite, Raid; maxId 62), so a truncated chain
//! silently loses Dungeon, World Event and Legendary while looking right on everything else — the
//! fixture test's row-count assert is what stands between us and that.
//!
//! **The `Type` field has a second reader in the client, and it is not a tag:** the
//! `PLAYER_QUEST_LOG` field watcher (`0x5dde6b`/`0x5ddf02`) tests it against `1` and fires
//! TriggerTutorial 40 when an *Elite* quest is accepted. Nothing else renders this table
//! (`0xc0d9cc` has one reader image-wide), so the tag belongs to the quest-log row and to nothing
//! else — but `+0x10` is not "the tag id".

use std::collections::HashMap;

use anyhow::Result;

use crate::chain::Chain;
use crate::dbc::load_id_name_table;

/// The resolved `Type → tag name` lookup ([`load_quest_tag_names`]).
#[derive(Debug, Default)]
pub struct QuestTagNames(HashMap<u32, String>);

impl QuestTagNames {
    /// The tag for a quest template's `Type`, or `None` for a type the table doesn't name —
    /// which includes `0`, the untagged majority. The caller renders nothing for `None`; it must
    /// not substitute an empty string, because the reference's row Lua branches on the tag's
    /// presence, not on its length.
    pub fn resolve(&self, quest_type: u32) -> Option<&str> {
        self.0.get(&quest_type).map(String::as_str)
    }

    /// How many tags the table names — the load's own sanity check (5875 ships 7).
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the table named nothing at all.
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
