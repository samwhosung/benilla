//! `ChrRaces.dbc` → the PvP team digit, the second `%d` of `PVP_RANK_<rank>_<team>` (`0x5efe00`):
//! race → `ChrRaces` column 2 (`[0xc0dee0]`) → that FactionTemplate's group mask (`[0xc0dd3c]`);
//! mask & 4 is 0 (Horde), mask & 2 is 1 (Alliance), else -1. It reads the race, unlike
//! `UnitFactionGroup`, whose live faction template (`0x5166b8`) loses its side under GM mode. The
//! runtime uses a frozen copy, `ui_unit::race_pvp_team`; this loader backs the test that pins it.

use std::collections::HashMap;

use anyhow::Result;

use crate::Chain;

/// Each `ChrRaces.dbc` row's team digit by race id; a race without a row (-1) is absent.
pub fn load_race_pvp_teams(chain: &mut Chain) -> Result<HashMap<u8, i8>> {
    let factions = crate::load_faction_catalog(chain)?;
    Ok(factions
        .race_templates()
        .map(|(race, template)| {
            // The Horde bit is tested first (`0x5efe42`), so a row with both bits reads Horde.
            let team = match factions.template(template).map(|t| t.group_mask) {
                Some(m) if m & 4 != 0 => 0,
                Some(m) if m & 2 != 0 => 1,
                _ => -1,
            };
            (race, team)
        })
        .collect())
}
