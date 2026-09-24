//! `WorldStateUI.dbc`, the rows behind the always-up PvP readout and the battleground scoreboard's
//! columns (`GetNumWorldStateUI`, `GetWorldStateUIInfo` `0x4c5a70`). It is client-only: vmangos
//! sends only world-state values, and which are shown, where and with what label is this table's.
//!
//! Layout: 39 fields of 156 bytes (the loader asserts both, `0x553a9e`, `0x553ad6`), each
//! localized string nine dwords; field 22 (`+0x58`) is loaded and never read, so not carried.

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const WORLD_STATE_UI: &str = "DBFilesClient\\WorldStateUI.dbc";

/// One `WorldStateUI.dbc` row.
#[derive(Clone, Debug)]
pub struct WorldStateUiRow {
    /// The row's `Map.dbc` id: a continent for world PvP, else a battleground map.
    pub map_id: u32,
    /// The `AreaTable.dbc` id narrowing the row within its map (world PvP only), `0` for all of it.
    pub area_id: u32,
    /// The static icon's texture path (`Interface\WorldStateFrame\AllianceTower`), or empty.
    pub icon: String,
    /// The label: a `%<id>w` world-state format (`"Towers Controlled: %2327w"`), or plain text on
    /// a scoreboard column. The only string the client expands; the rest reach Lua verbatim.
    pub text: String,
    /// The tooltip line (`"Alliance Towers Controlled"`).
    pub tooltip: String,
    /// `StateVariable` (`+0x5c`), the world-state id whose value the client returns as the row's
    /// `uiState`; `0` is no state, which the binding answers as `1.0`.
    pub state_variable: u32,
    /// `Type` (`+0x60`), which list the row joins: `0` always up; `1` always up only while the
    /// player is in a zone-dependent defense channel (`ZONE_DEP | DEFENSE`, LocalDefense alone),
    /// the world-PvP rows; `2` a scoreboard column, built by another handler.
    pub ui_type: u32,
    /// The icon shown instead while [`Self::state_variable`] is live: the enemy flag, on the two
    /// Warsong Gulch rows that have one.
    pub dynamic_icon: String,
    /// The tooltip that goes with [`Self::dynamic_icon`] (`"Horde flag has been picked up"`).
    pub dynamic_tooltip: String,
    /// A token naming an extra widget the row drives: `"CAPTUREPOINT"`, on the Eastern
    /// Plaguelands progress row alone.
    pub extended_ui: String,
    /// The world-state ids that widget reads; the binding answers their values.
    pub extended_ui_state: [u32; 3],
}

/// `WorldStateUI.dbc` rows in file order, with their ids.
pub struct WorldStateUiCatalog {
    rows: Vec<(u32, WorldStateUiRow)>,
}

impl WorldStateUiCatalog {
    /// A catalog over rows given directly, for tests that need a table of known shape.
    pub fn from_rows(rows: Vec<(u32, WorldStateUiRow)>) -> Self {
        Self { rows }
    }

    /// Every row in file order, the order a UI list built from the table is indexed by.
    pub fn rows(&self) -> impl Iterator<Item = (u32, &WorldStateUiRow)> {
        self.rows.iter().map(|(id, row)| (*id, row))
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("WorldStateUI");
    for name in ["ID", "MapID", "AreaID"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s.add_field(SchemaField::new("Icon", FieldType::String));
    let loc = |s: &mut Schema, name: &str| {
        s.add_field(SchemaField::new(name, FieldType::String)); // enUS (locale 0)
        s.add_field(SchemaField::new_array(
            format!("{name}OtherLocales"),
            FieldType::String,
            7,
        ));
        s.add_field(SchemaField::new(format!("{name}Flags"), FieldType::UInt32));
    };
    loc(&mut s, "Text");
    loc(&mut s, "Tooltip");
    // +0x58: loaded, never read.
    for name in ["UnreadCol22", "StateVariable", "Type"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s.add_field(SchemaField::new("DynamicIcon", FieldType::String));
    loc(&mut s, "DynamicTooltip");
    s.add_field(SchemaField::new("ExtendedUI", FieldType::String));
    s.add_field(SchemaField::new_array(
        "ExtendedUIStateVariable",
        FieldType::UInt32,
        3,
    ));
    s
}

/// Read `WorldStateUI.dbc` off the patch chain.
pub fn load_world_state_ui_catalog(chain: &mut Chain) -> Result<WorldStateUiCatalog> {
    let bytes = chain
        .read_file(WORLD_STATE_UI)
        .context("reading WorldStateUI.dbc")?;
    let rs = parse(&bytes, schema(), "WorldStateUI")?;
    let mut rows = Vec::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        rows.push((
            id,
            WorldStateUiRow {
                map_id: u32_at(r, 1).unwrap_or(0),
                area_id: u32_at(r, 2).unwrap_or(0),
                icon: str_at(&rs, r, 3).unwrap_or_default(),
                text: str_at(&rs, r, 4).unwrap_or_default(),
                tooltip: str_at(&rs, r, 13).unwrap_or_default(),
                state_variable: u32_at(r, 23).unwrap_or(0),
                ui_type: u32_at(r, 24).unwrap_or(0),
                dynamic_icon: str_at(&rs, r, 25).unwrap_or_default(),
                dynamic_tooltip: str_at(&rs, r, 26).unwrap_or_default(),
                extended_ui: str_at(&rs, r, 35).unwrap_or_default(),
                extended_ui_state: [
                    u32_at(r, 36).unwrap_or(0),
                    u32_at(r, 37).unwrap_or(0),
                    u32_at(r, 38).unwrap_or(0),
                ],
            },
        ));
    }
    Ok(WorldStateUiCatalog { rows })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_world_state_ui_layout_sanity() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_world_state_ui_catalog(&mut chain).expect("load WorldStateUI");
        assert_eq!(cat.len(), 20, "all 20 rows load");

        let by_id = |id: u32| {
            cat.rows()
                .find(|(r, _)| *r == id)
                .map(|(_, row)| row)
                .unwrap_or_else(|| panic!("row {id}"))
        };

        // ── The Eastern Plaguelands pair.
        let alliance = by_id(136);
        assert_eq!((alliance.map_id, alliance.area_id), (0, 139));
        assert_eq!(alliance.icon, "Interface\\WorldStateFrame\\AllianceTower");
        assert_eq!(alliance.text, "Towers Controlled: %2327w");
        assert_eq!(alliance.tooltip, "Alliance Towers Controlled");
        assert_eq!(alliance.ui_type, 1);
        let horde = by_id(137);
        assert_eq!(horde.icon, "Interface\\WorldStateFrame\\HordeTower");
        assert_eq!(horde.text, "Towers Controlled: %2328w");
        assert_eq!((alliance.ui_type, horde.ui_type), (1, 1));
        assert_eq!(
            (alliance.state_variable, horde.state_variable),
            (0, 0),
            "no uiState of their own — the binding answers 1.0"
        );

        // ── The one extended-UI row, and the only user of the trailing id array.
        let progress = by_id(138);
        assert_eq!(progress.extended_ui, "CAPTUREPOINT");
        assert_eq!(progress.extended_ui_state, [2427, 2428, 0]);
        assert_eq!(progress.state_variable, 2426);
        assert_eq!(progress.text, "Progress: %2427w");

        // ── Warsong Gulch: the dynamic-icon columns, the only rows that carry them.
        let ws_alliance = by_id(2);
        assert_eq!(ws_alliance.map_id, 489);
        assert_eq!(ws_alliance.state_variable, 2339);
        assert_eq!(
            ws_alliance.dynamic_icon,
            "Interface\\WorldStateFrame\\HordeFlag"
        );
        assert_eq!(ws_alliance.dynamic_tooltip, "Horde flag has been picked up");
        assert_eq!(ws_alliance.text, "%1581w/%1601w");
        assert_eq!(ws_alliance.ui_type, 0, "unconditional always-up");

        // ── An Alterac Valley scoreboard column: type 2, plain text rather than a format.
        let graveyards = by_id(63);
        assert_eq!(graveyards.map_id, 30);
        assert_eq!(graveyards.ui_type, 2);
        assert_eq!(graveyards.text, "Graveyards Assaulted");
        assert!(!graveyards.text.contains('%'));

        // ── Whole-table shape.
        let mut with_area = 0;
        let mut with_macro = 0;
        for (_, row) in cat.rows() {
            assert!(
                matches!(row.ui_type, 0..=2),
                "Type is a small enum: {}",
                row.ui_type
            );
            assert!(
                row.state_variable == 0 || (2000..3000).contains(&row.state_variable),
                "StateVariable lands in the live world-state id band: {}",
                row.state_variable
            );
            assert!(!row.text.is_empty(), "every row is labelled");
            assert_ne!(
                row.map_id,
                u32::MAX,
                "the MapID -1 wildcard has no shipped row — every row names a real map"
            );
            assert!(
                row.icon.is_empty() || row.icon.starts_with("Interface\\"),
                "Icon is a texture path or nothing: {:?}",
                row.icon
            );
            if row.area_id != 0 {
                with_area += 1;
            }
            if row.text.contains('%') {
                with_macro += 1;
            }
        }
        assert_eq!(with_area, 5, "only the two outdoor zones narrow by area");
        assert_eq!(
            with_macro, 9,
            "the always-up rows read the world-state table"
        );
    }
}
