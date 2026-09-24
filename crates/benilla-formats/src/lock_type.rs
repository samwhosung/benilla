//! LockType.dbc — the small table that names a lock's *interaction kind* (Herbalism, Mining, Pick
//! Lock, Fishing, …), and — for the three that carry one — the **cursor** the client shows when you
//! hover a GameObject wearing that lock (decision 0236; `0x5f3070`). The world
//! cursor's GameObject branch resolves a base-type GO's cursor by data: the GO template's `lockId` →
//! [`crate::LockCatalog`] row → its **first** requirement slot's `LockType` index → *this* table's
//! **CursorName** column. A non-empty CursorName (only `PickLock`/`GatherHerbs`/`Mine` in 5875) names
//! the cursor BLP directly; an empty one (every other lock kind — a plain chest, a door, fishing)
//! falls through to the generic **Interact** gear. `LockType.Id == 1` (Pick Lock) is additionally the
//! "never grayed" case the classifier special-cases.
//!
//! The localized **Name** block (`Name@1..8` + flags, enUS at field 1 — `[lockTypeRow + locale*4 +
//! 4]`, the exact read the lock-refusal toast performs at `0x5f34f9`) is the word the client fills
//! into the client-local "Requires %s" error for an unopenable skill lock — "Requires Herbalism" /
//! "Requires Mining" (decision 0545).
//!
//! Layout verified against build 5875 (byte-checked live: 19 records × 29 fields, record size 116):
//! `ID@0`, then the localized `Name` block, and **`CursorName@28`** (`[lockTypeRow+0x70]`, the exact
//! offset `0x5f3070` reads). Every field is 4 bytes, so the intervening columns are read as
//! `UInt32` filler — only `ID`, `Name` (enUS), and `CursorName` are consumed here.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};

const LOCK_TYPE: &str = "DBFilesClient\\LockType.dbc";
/// The file's column count (must equal the DBC header `field_count` — `benilla-dbc` enforces it).
const LOCK_TYPE_FIELDS: usize = 29;
/// The **CursorName** column (`[lockTypeRow+0x70]` = field 28, VERIFIED both by the offset
/// `0x5f3070` reads and by a live byte-dump of the 5875 file).
const CURSOR_NAME_FIELD: usize = 28;
/// The localized **Name** block's enUS column (field 1 — the toast's `[lockTypeRow + locale*4 + 4]`
/// with locale 0).
const NAME_FIELD: usize = 1;

/// `LockType.Id → CursorName` for the rows that carry one — the data half of the GameObject cursor —
/// plus every row's localized `Name` (the lock-refusal toast's "Requires %s" fill).
/// Only the three interaction kinds with a distinct cursor keep one (`PickLock`/`GatherHerbs`/`Mine`);
/// every other row's CursorName is empty and simply absent here (→ the generic Interact gear).
pub struct LockTypeCatalog {
    cursors: HashMap<u32, String>,
    names: HashMap<u32, String>,
}

impl LockTypeCatalog {
    /// The `Interface\Cursor\<name>.blp` stem for a `LockType` index, or `None` when that lock kind
    /// carries no distinct cursor (→ the caller falls back to the Interact gear). The index is the
    /// value the client reads straight out of the lock row's first requirement slot; a value that
    /// isn't a real LockType id (e.g. a key-item entry, or `0` for an empty slot) simply misses.
    pub fn cursor_name(&self, lock_type_id: u32) -> Option<&str> {
        self.cursors.get(&lock_type_id).map(String::as_str)
    }

    /// The localized `Name` of a lock kind ("Herbalism", "Mining", "Pick Lock", …) — the word the
    /// lock-refusal toast fills into "Requires %s" (`0x5f34f9`; the client falls
    /// back to the literal `"UNKNOWN"` when the row is missing — that fallback is the caller's).
    pub fn name(&self, lock_type_id: u32) -> Option<&str> {
        self.names.get(&lock_type_id).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.cursors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cursors.is_empty()
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("LockType");
    for i in 0..LOCK_TYPE_FIELDS {
        // Only ID(0), Name-enUS(1) and CursorName(28) are read; the rest are 4-byte filler
        // (localization columns) declared UInt32 so the record size still matches the file header.
        let ty = if i == CURSOR_NAME_FIELD || i == NAME_FIELD {
            FieldType::String
        } else {
            FieldType::UInt32
        };
        s.add_field(SchemaField::new(format!("F{i}"), ty));
    }
    s
}

/// Load LockType.dbc from the patch chain into a [`LockTypeCatalog`].
pub fn load_lock_type_catalog(chain: &mut Chain) -> Result<LockTypeCatalog> {
    let bytes = chain
        .read_file(LOCK_TYPE)
        .with_context(|| format!("reading {LOCK_TYPE}"))?;
    let rs = parse(&bytes, schema(), "LockType.dbc")?;
    let mut cursors = HashMap::new();
    let mut names = HashMap::new();
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        if let Some(name) = str_at(&rs, r, CURSOR_NAME_FIELD) {
            cursors.insert(id, name);
        }
        if let Some(name) = str_at(&rs, r, NAME_FIELD) {
            names.insert(id, name);
        }
    }
    Ok(LockTypeCatalog { cursors, names })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three cursor-bearing rows on the real build-5875 `LockType.dbc`, byte-anchored: a column
    /// slip (or an off-by-one on the CursorName offset) lands on empty/other columns and fails loudly.
    /// Skips without client data.
    #[test]
    fn real_lock_types_name_their_cursors() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_lock_type_catalog(&mut chain).expect("load LockType.dbc");

        // The full cursor-bearing set in 5875 — Pick Lock (1), Herbalism (2), Mining (3).
        assert_eq!(cat.cursor_name(1), Some("PickLock"));
        assert_eq!(cat.cursor_name(2), Some("GatherHerbs"));
        assert_eq!(cat.cursor_name(3), Some("Mine"));
        // Every other lock kind (Open, Fishing, Disarm Trap, …) carries no cursor → Interact gear.
        assert_eq!(cat.cursor_name(5), None); // Open (the keyless-chest LockType 13 is likewise blank)
        assert_eq!(cat.cursor_name(13), None);
        assert_eq!(cat.cursor_name(19), None); // Fishing
        assert_eq!(cat.cursor_name(0), None); // an empty lock slot's index
        assert_eq!(
            cat.len(),
            3,
            "only three LockType rows carry a CursorName in 5875"
        );

        // The localized Name column — the lock-refusal toast's "Requires %s" fill (decision 0545).
        // A column slip here would print e.g. "Requires Herbs" (ResourceName@10) on screen.
        assert_eq!(cat.name(1), Some("Pick Lock"));
        assert_eq!(cat.name(2), Some("Herbalism"));
        assert_eq!(cat.name(3), Some("Mining"));
        assert_eq!(cat.name(19), Some("Fishing"));
        assert_eq!(cat.name(0), None);
    }
}
