//! `Stationery.dbc`: a mail's `stationery` id (`SMSG_MAIL_LIST_RESULT`) → the texture basename
//! `OpenMail_Update` paints as `Interface\Stationery\<basename>1` and `2`, the left and right
//! halves. The send side lists a row when its item's template is cached and either `Flags & 1`
//! (`0x4aca2a`) or the player carries its `ItemID` (`0x4aca1c`); the list sorts by buy price.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::chain::Chain;
use crate::dbc::{parse, str_at, u32_at};

const STATIONERY: &str = "DBFilesClient\\Stationery.dbc";

/// vmangos `MAIL_STATIONERY_DEFAULT`; the server stores every player mail with it.
pub const STATIONERY_DEFAULT: u32 = 41;

/// One `Stationery.dbc` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StationeryRow {
    /// The stationery id a mail carries on the wire and `SelectStationery` stores.
    pub id: u32,
    /// The item that makes this paper usable when carried.
    pub item: u32,
    pub texture: String,
    /// `& 1`: always available, carried or not.
    pub flags: u32,
}

/// Stationery id → texture basename, plus the rows for the send side's list.
pub struct StationeryCatalog {
    rows: Vec<StationeryRow>,
    by_id: HashMap<u32, String>,
}

impl StationeryCatalog {
    /// Id 41's `Texture` in the shipped file, the fallback for an unknown id or a failed load.
    pub const DEFAULT_TEXTURE: &'static str = "STATIONERYTEST";

    /// The basename for a stationery id, or [`Self::DEFAULT_TEXTURE`] for one the table lacks.
    pub fn texture(&self, id: u32) -> &str {
        self.by_id
            .get(&id)
            .map(String::as_str)
            .unwrap_or(Self::DEFAULT_TEXTURE)
    }

    /// The basename with no fallback, as `GetSelectedStationeryTexture` answers.
    pub fn texture_of(&self, id: u32) -> Option<&str> {
        self.by_id.get(&id).map(String::as_str)
    }

    /// Every row, in DBC order.
    pub fn rows(&self) -> &[StationeryRow] {
        &self.rows
    }
}

/// Load `Stationery.dbc` from the patch chain.
pub fn load_stationery_catalog(chain: &mut Chain) -> Result<StationeryCatalog> {
    let bytes = chain
        .read_file(STATIONERY)
        .context("reading Stationery.dbc")?;
    let mut schema = Schema::new("Stationery");
    schema.add_field(SchemaField::new("ID", FieldType::UInt32));
    schema.add_field(SchemaField::new("Item", FieldType::UInt32));
    schema.add_field(SchemaField::new("Texture", FieldType::String));
    schema.add_field(SchemaField::new("Flags", FieldType::UInt32));
    let set = parse(&bytes, schema, "Stationery.dbc")?;
    let mut rows = Vec::new();
    let mut by_id = HashMap::new();
    for r in set.records() {
        if let (Some(id), Some(item), Some(tex), Some(flags)) =
            (u32_at(r, 0), u32_at(r, 1), str_at(&set, r, 2), u32_at(r, 3))
        {
            by_id.insert(id, tex.clone());
            rows.push(StationeryRow {
                id,
                item,
                texture: tex,
                flags,
            });
        }
    }
    Ok(StationeryCatalog { rows, by_id })
}
