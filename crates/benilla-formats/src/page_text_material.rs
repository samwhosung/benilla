//! `PageTextMaterial.dbc` — the book-frame material lookup.
//!
//! A readable carries a material *id* (a book item template's `PageMaterial`, a
//! `GAMEOBJECT_TYPE_TEXT` object's template `data[2]`); the reference resolves it to a **basename**
//! through this table and hands that string to Lua as `ItemTextGetMaterial()`, which paints the
//! four corner textures from `Interface\ItemTextFrame\ItemText-<basename>-TopLeft` … and picks the
//! page's font/text colour (`ItemTextFrame.lua` l.63-104). `"Parchment"` is the special one: the
//! Lua hides the corner art for it, and it is also the substitute when the getter returns nothing.
//!
//! **The resolve is byte-verified** (`ItemTextGetMaterial 0x4e39f0`): the GameObject leg calls
//! `0x5f5950` — template-attribute `0x11` → the type's `data[]` slot → the id — then bounds-checks
//! `0 < id <= [0xc0da20]` against the DBC's id-indexed table `[0xc0da1c]` and returns `[row+4]`,
//! the row's **second** field. Out of range, or `id <= 0`, pushes `nil` (→ the Lua's Parchment).
//!
//! The 5875 schema was read byte-level from the real file (VERIFIED at decision time): WDBC header
//! `record_count = 6`, `field_count = 2`, `record_size = 8`, string block 48 bytes — two 4-byte
//! fields, the second a string ref. The six rows are `1 Parchment · 2 Stone · 3 Marble · 4 Silver ·
//! 5 Bronze · 6 Valentine`.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::chain::Chain;
use crate::dbc::{parse, str_at, u32_at};

const PAGE_TEXT_MATERIAL: &str = "DBFilesClient\\PageTextMaterial.dbc";

/// `PageTextMaterial.dbc`: material id → basename (the `ItemText-<basename>-<corner>` stem).
pub struct PageTextMaterialCatalog {
    by_id: HashMap<u32, String>,
}

impl PageTextMaterialCatalog {
    /// A catalog from `(id, basename)` rows — the shipped six are 1 Parchment · 2 Stone · 3 Marble
    /// · 4 Silver · 5 Bronze · 6 Valentine (measured on the shipped file).
    pub fn from_rows(rows: &[(u32, &str)]) -> Self {
        Self {
            by_id: rows.iter().map(|&(id, n)| (id, n.to_string())).collect(),
        }
    }

    /// The basename for a material id, or `None` for `0`/an id the table doesn't carry — the
    /// reference's own `nil`, which `ItemTextFrame.lua` substitutes with "Parchment" (and then
    /// hides the corner art, since Parchment has none).
    pub fn name(&self, id: u32) -> Option<&str> {
        self.by_id.get(&id).map(String::as_str)
    }
}

/// Load `PageTextMaterial.dbc` into an id → basename map (see the module doc for the verified
/// schema).
pub fn load_page_text_material_catalog(chain: &mut Chain) -> Result<PageTextMaterialCatalog> {
    let bytes = chain
        .read_file(PAGE_TEXT_MATERIAL)
        .context("reading PageTextMaterial.dbc")?;
    let mut schema = Schema::new("PageTextMaterial");
    schema.add_field(SchemaField::new("ID", FieldType::UInt32));
    schema.add_field(SchemaField::new("Name", FieldType::String));
    let set = parse(&bytes, schema, "PageTextMaterial.dbc")?;
    let mut by_id = HashMap::new();
    for r in set.records() {
        if let (Some(id), Some(name)) = (u32_at(r, 0), str_at(&set, r, 1)) {
            by_id.insert(id, name);
        }
    }
    Ok(PageTextMaterialCatalog { by_id })
}
