//! `ChrRaces.dbc` → the **PvP team digit** — `0x5efe00`, the second `%d` of
//! `PVP_RANK_<rank>_<team>`.
//!
//! The engine never asks a unit what faction template it is *currently* carrying for this; it asks
//! what RACE it is, and walks the race's own table:
//!
//! ```text
//! 5efe06  movzx eax, BYTE PTR [[obj+0x110]+0x78]   ; UNIT_FIELD_BYTES_0 byte 0 = race
//!         -> ChrRaces  [0xc0dee0][race]   +0x08    ; field 2 = FactionTemplate id
//!         -> FactionTemplate [0xc0dd3c][id] +0x0c  ; field 3 = factionGroupMask
//! 5efe42  test al,4 ; je ...                       ; mask & 4 -> 0 (Horde)
//! 5efe49  and al,2 ; …                             ; mask & 2 -> 1 (Alliance), else -1
//! 5efe54  (any bound/NULL failure)                 -> -1
//! ```
//!
//! (The `0`/`1` assignment is settled against the shipped `FactionGroup.dbc` and
//! `GlobalStrings.lua`, not inferred.)
//!
//! **This is not `UnitFactionGroup`**, which reads the live `UNIT_FIELD_FACTIONTEMPLATE`
//! (`0x5166b8`/`0x5166be`) and so genuinely loses its side under GM mode. Confusing the two is
//! report B378 /.
//!
//! Loaded only by [`crate::race_pvp_team::load_race_pvp_teams`]'s one consumer — the test that
//! pins `ui_unit::race_pvp_team`'s frozen table to the shipped tables. The runtime answer is that
//! table: nine constant rows of a 2006 file are not worth a DBC resource threaded through every
//! unit snapshot, but they are worth a test that fails if the file ever disagrees.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
use crate::Chain;

const CHR_RACES: &str = "DBFilesClient\\ChrRaces.dbc";

/// Every `ChrRaces.dbc` row's team digit, by race id — `0` Horde, `1` Alliance, `-1` no side.
///
/// The map holds one entry per row the file actually has (nine in 5875, ids 1–9); a race id with
/// no row is the engine's `-1` tail and is simply absent here.
pub fn load_race_pvp_teams(chain: &mut Chain) -> Result<HashMap<u8, i8>> {
    let bytes = chain
        .read_file(CHR_RACES)
        .with_context(|| format!("reading {CHR_RACES}"))?;
    // 29 columns in 5875; the string columns are declared here only so the field count matches —
    // this loader reads none of them. Same schema as `crate::characters`' creation catalog.
    let mut schema = Schema::new("ChrRaces");
    for i in 0..29 {
        let ty = match i {
            15 | 26 | 27 | 28 => FieldType::String,
            _ => FieldType::UInt32,
        };
        schema.add_field(SchemaField::new(format!("f{i}"), ty));
    }
    let rs = parse(&bytes, schema, "ChrRaces")?;
    let factions = crate::load_faction_catalog(chain)?;
    let mut out = HashMap::new();
    for r in rs.records() {
        let (Some(race), Some(template)) = (u32_at(r, 0), u32_at(r, 2)) else {
            continue;
        };
        let Ok(race) = u8::try_from(race) else {
            continue;
        };
        // `0x5efe42`'s order: the Horde bit is tested FIRST, so a hypothetical row carrying both
        // side bits would read Horde. Reproduced rather than tidied.
        let team = match factions.template(template).map(|t| t.group_mask) {
            Some(m) if m & 4 != 0 => 0,
            Some(m) if m & 2 != 0 => 1,
            _ => -1,
        };
        out.insert(race, team);
    }
    Ok(out)
}
