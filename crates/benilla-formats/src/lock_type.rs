//! `LockType.dbc`, a lock's interaction kind (Herbalism, Mining, Pick Lock, Fishing): its name and,
//! for three kinds, the cursor shown over a GameObject wearing it (`0x5f3070`). The GO cursor
//! resolves by data: template `lockId` → [`crate::LockCatalog`] row → its first slot's `LockType`
//! index → `CursorName`, which names the cursor BLP, or when empty falls through to the Interact
//! gear. `LockType` 1 (Pick Lock) is also the classifier's never-grayed case.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};

const LOCK_TYPE: &str = "DBFilesClient\\LockType.dbc";
/// The file's column count, which `benilla-dbc` checks against the header.
const LOCK_TYPE_FIELDS: usize = 29;
/// `CursorName`, the `[row+0x70]` that `0x5f3070` reads.
const CURSOR_NAME_FIELD: usize = 28;
/// The `Name` block's enUS column, the toast's `[row + locale*4 + 4]` at locale 0.
const NAME_FIELD: usize = 1;

/// `LockType.Id` → `CursorName` for the three rows with one, plus every row's `Name`.
pub struct LockTypeCatalog {
    cursors: HashMap<u32, String>,
    names: HashMap<u32, String>,
}

impl LockTypeCatalog {
    /// The `Interface\Cursor\<name>.blp` stem for a `LockType` index, `None` for the Interact
    /// gear. The index is the lock's raw first slot, so a key item or an empty `0` simply misses.
    pub fn cursor_name(&self, lock_type_id: u32) -> Option<&str> {
        self.cursors.get(&lock_type_id).map(String::as_str)
    }

    /// A lock kind's `Name` ("Herbalism"), filled into "Requires %s" (`0x5f34f9`); the caller
    /// supplies the reference's `"UNKNOWN"` for a missing row.
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
        // The unread columns are 4-byte filler, declared so the record size matches the header.
        let ty = if i == CURSOR_NAME_FIELD || i == NAME_FIELD {
            FieldType::String
        } else {
            FieldType::UInt32
        };
        s.add_field(SchemaField::new(format!("F{i}"), ty));
    }
    s
}

/// Load `LockType.dbc` from the patch chain into a [`LockTypeCatalog`].
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

    /// The 5875 rows: a column slip lands on another column and fails.
    #[test]
    fn real_lock_types_name_their_cursors() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_lock_type_catalog(&mut chain).expect("load LockType.dbc");

        assert_eq!(cat.cursor_name(1), Some("PickLock"));
        assert_eq!(cat.cursor_name(2), Some("GatherHerbs"));
        assert_eq!(cat.cursor_name(3), Some("Mine"));
        assert_eq!(cat.cursor_name(5), None); // Open; 13 is the keyless chest
        assert_eq!(cat.cursor_name(13), None);
        assert_eq!(cat.cursor_name(19), None); // Fishing
        assert_eq!(cat.cursor_name(0), None); // an empty lock slot's index
        assert_eq!(
            cat.len(),
            3,
            "only three LockType rows carry a CursorName in 5875"
        );

        // A slip to `ResourceName` (field 10) would print "Requires Herbs".
        assert_eq!(cat.name(1), Some("Pick Lock"));
        assert_eq!(cat.name(2), Some("Herbalism"));
        assert_eq!(cat.name(3), Some("Mining"));
        assert_eq!(cat.name(19), Some("Fishing"));
        assert_eq!(cat.name(0), None);
    }
}
