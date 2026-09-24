//! `PageTextMaterial.dbc`: a readable's material id (a book's `PageMaterial`, a text object's
//! `data[2]`) → the basename `ItemTextGetMaterial()` (`0x4e39f0`) hands to Lua, which paints the
//! `ItemText-<basename>-*` corners and picks the text colour (`ItemTextFrame.lua:63-104`). An id
//! outside `0 < id <= [0xc0da20]` returns nil, which the Lua shows as "Parchment", without corners.

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
    /// A catalog from `(id, basename)` rows. The file ships 1 Parchment, 2 Stone, 3 Marble,
    /// 4 Silver, 5 Bronze, 6 Valentine.
    pub fn from_rows(rows: &[(u32, &str)]) -> Self {
        Self {
            by_id: rows.iter().map(|&(id, n)| (id, n.to_string())).collect(),
        }
    }

    /// The basename for a material id; `None` where the reference returns `nil`.
    pub fn name(&self, id: u32) -> Option<&str> {
        self.by_id.get(&id).map(String::as_str)
    }
}

/// Load `PageTextMaterial.dbc` from the patch chain.
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
