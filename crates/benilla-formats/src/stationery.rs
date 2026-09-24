//! `Stationery.dbc` — the mail-window's letter-backdrop lookup (decision 0544).
//!
//! A mail carries a `stationery` id on the wire (`SMSG_MAIL_LIST_RESULT`); the reference client
//! resolves that id to a texture *basename* through this table and paints the open-letter backdrop
//! from `Interface\Stationery\<basename>1` (left half) + `<basename>2` (right half) — see
//! `MailFrame.lua`'s `OpenMail_Update` (`STATIONERY_PATH..texture.."1"/"2"`).
//!
//! The 5875 schema was read byte-level from the real `patch.MPQ` file (VERIFIED at decision time):
//! WDBC header `record_count = 5`, `field_count = 4`, `record_size = 16`, string block 62 bytes —
//! four 4-byte fields, the third a string ref: `{ID, ItemID, Texture, Flags}`. Fields 1 and 3
//! are not filler (decision 1970): `ItemID` (`0x4aca1c`) is the stationery ITEM the player buys
//! or carries to use the paper, and `Flags & 1` (`0x4aca2a`) marks the one always available
//! (`41 Default Stationery`, BuyPrice 0). The client's usable list
//! is `(Flags & 1 || the player carries ItemID) && the item's template is cached`, sorted by
//! BuyPrice ascending — the `GetNumStationeries`/`GetStationeryInfo` surface. The verified rows:
//! `1/41 → STATIONERYTEST`, `61 → GMSTATIONERY`, `62 → AUCTIONSTATIONERY`, `64 → STATIONERY_VAL`
//! (both target BLPs exist in the archive; MPQ path lookup is case-insensitive, so the uppercase
//! DBC string resolves the mixed-case file).

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::chain::Chain;
use crate::dbc::{parse, str_at, u32_at};

const STATIONERY: &str = "DBFilesClient\\Stationery.dbc";

/// `MAIL_STATIONERY_DEFAULT` — vmangos stores every player mail with this id (the client's
/// stationery choice is discarded server-side, decision 0544). Its verified texture basename is the
/// [`StationeryCatalog::DEFAULT_TEXTURE`] fallback.
pub const STATIONERY_DEFAULT: u32 = 41;

/// One `Stationery.dbc` row (the module doc's four columns).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StationeryRow {
    /// The stationery id a mail carries on the wire and `SelectStationery` stores.
    pub id: u32,
    /// The stationery item — bought or carried to use this paper.
    pub item: u32,
    /// The texture basename (the `Interface\Stationery\<basename>N` stem).
    pub texture: String,
    /// `& 1`: always available, carried or not.
    pub flags: u32,
}

/// `Stationery.dbc`: stationery id → texture basename (the `Interface\Stationery\<basename>N` stem),
/// and the rows whole for the send side's usable list.
pub struct StationeryCatalog {
    rows: Vec<StationeryRow>,
    by_id: HashMap<u32, String>,
}

impl StationeryCatalog {
    /// The verified basename of the default stationery (id 41) — the fallback when a mail's id is
    /// missing from the table or the DBC failed to load. VERIFIED against the real file's string
    /// block (id 41's actual `Texture` value), NOT an assumed constant.
    pub const DEFAULT_TEXTURE: &'static str = "STATIONERYTEST";

    /// The texture basename for a stationery id, falling back to [`Self::DEFAULT_TEXTURE`] for any
    /// id the table doesn't carry (unknown/AH/creature stationery still renders a valid backdrop).
    pub fn texture(&self, id: u32) -> &str {
        self.by_id
            .get(&id)
            .map(String::as_str)
            .unwrap_or(Self::DEFAULT_TEXTURE)
    }

    /// The texture basename for a stationery id the table carries — `None` for an id gap, which
    /// is what `GetSelectedStationeryTexture` answers with (no default there).
    pub fn texture_of(&self, id: u32) -> Option<&str> {
        self.by_id.get(&id).map(String::as_str)
    }

    /// Every row, in DBC order.
    pub fn rows(&self) -> &[StationeryRow] {
        &self.rows
    }
}

/// Load `Stationery.dbc` into an id → basename map (see the module doc for the verified schema).
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
