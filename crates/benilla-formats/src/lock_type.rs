//! `LockType.dbc`, a lock's interaction kind (Herbalism, Mining, Pick Lock, Fishing): its name and,
//! for three kinds, the cursor shown over a GameObject wearing it (`0x5f3070`). The GO cursor
//! resolves by data: template `lockId` → [`crate::LockCatalog`] row → its first slot's `LockType`
//! index → `CursorName`, which names the cursor BLP, or when empty falls through to the Interact
//! gear. `LockType` 1 (Pick Lock) is also the classifier's never-grayed case.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{loc_str_raw, parse, str_at, u32_at};

const LOCK_TYPE: &str = "DBFilesClient\\LockType.dbc";
/// The file's column count, which `benilla-dbc` checks against the header.
const LOCK_TYPE_FIELDS: usize = 29;
/// `CursorName`, the `[row+0x70]` that `0x5f3070` reads.
const CURSOR_NAME_FIELD: usize = 28;
/// The `Name` block's first column, the toast's `[row + locale*4 + 4]`.
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
    /// supplies the reference's `"UNKNOWN"` for a missing row, and a blank name is none here.
    pub fn name(&self, lock_type_id: u32) -> Option<&str> {
        self.names
            .get(&lock_type_id)
            .map(String::as_str)
            .filter(|n| !n.is_empty())
    }

    /// Every row's `Name`, by `LockType` index, a blank one included: the item tooltip answers
    /// `UNKNOWN` only for a row that is missing (`0x52bede`..`0x52bef2`), else the name as it is,
    /// blank too (`0x52bef4`).
    pub fn names(&self) -> impl Iterator<Item = (u32, &str)> {
        self.names.iter().map(|(&id, name)| (id, name.as_str()))
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
    catalog_from_bytes(&bytes, chain.locale())
}

fn catalog_from_bytes(bytes: &[u8], locale: crate::Locale) -> Result<LockTypeCatalog> {
    let rs = parse(bytes, schema(), "LockType.dbc")?;
    let mut cursors = HashMap::new();
    let mut names = HashMap::new();
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        if let Some(name) = str_at(&rs, r, CURSOR_NAME_FIELD) {
            cursors.insert(id, name);
        }
        if let Some(name) = loc_str_raw(&rs, r, NAME_FIELD, locale) {
            names.insert(id, name);
        }
    }
    Ok(LockTypeCatalog { cursors, names })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two WDBC rows of the shipped shape: type 1 named, type 2 with a blank name column.
    fn synthetic() -> Vec<u8> {
        let strings = b"\0Pick Lock\0".to_vec();
        let mut bytes = b"WDBC".to_vec();
        for w in [
            2,
            LOCK_TYPE_FIELDS as u32,
            LOCK_TYPE_FIELDS as u32 * 4,
            strings.len() as u32,
        ] {
            bytes.extend_from_slice(&w.to_le_bytes());
        }
        for (id, name_at) in [(1u32, 1u32), (2, 0)] {
            let mut row = vec![0u32; LOCK_TYPE_FIELDS];
            row[0] = id;
            row[NAME_FIELD] = name_at;
            for f in row {
                bytes.extend_from_slice(&f.to_le_bytes());
            }
        }
        bytes.extend_from_slice(&strings);
        bytes
    }

    /// A blank `Name` is a row with no word, not a missing row: the item tooltip answers `UNKNOWN`
    /// only for a missing row (`0x52bede`..`0x52bef2`) and the empty string for a blank one
    /// (`0x52bef4`), so [`LockTypeCatalog::names`] lists it; the toast's [`LockTypeCatalog::name`]
    /// still reads it as none.
    #[test]
    fn a_blank_name_is_a_row_not_a_missing_one() {
        let locale = crate::Locale::from_index(0).unwrap();
        let cat = catalog_from_bytes(&synthetic(), locale).unwrap();
        let mut names: Vec<_> = cat.names().collect();
        names.sort();
        assert_eq!(names, vec![(1, "Pick Lock"), (2, "")]);
        assert_eq!(cat.name(1), Some("Pick Lock"));
        assert_eq!(cat.name(2), None);
    }

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
