//! `WorldStateUI.dbc` — the rows behind the always-up PvP status readout and the battleground
//! scoreboard's columns (`GetNumWorldStateUI` / `GetWorldStateUIInfo`).
//!
//! **Client-only: vmangos carries no struct for this table.** The server's whole contribution is
//! the world-state *values* (`SMSG_INIT_WORLD_STATES` / `SMSG_UPDATE_WORLD_STATE`,
//! [`crate::messages::world_state`](benilla_protocol) on the wire); which of them are *displayed*,
//! where, and with what label is entirely this file's.
//!
//! Layout pinned by inspection of the shipped 5875 file (2026-08-25): **20 × 39 × 156 B**. The
//! field boundaries are not assumed from a later build's schema — they are proven by the
//! localized-string signature, which could have come out otherwise: a loc block in this era is
//! nine dwords (the enUS string + seven other-locale slots + a flags dword), and a census of all
//! 20 rows finds fields 5-11, 14-20 and 27-33 **uniformly zero** with 12, 21 and 34 each holding a
//! locale-flags mask (`0x3F00BE` and two siblings). That fixes three loc blocks at 4-12, 13-21 and
//! 26-34, and everything between them follows:
//!
//! `ID(0), MapID(1), AreaID(2), Icon(3, a path string), Text_lang(4-12), Tooltip_lang(13-21),
//! <unread>(22), StateVariable(23), Type(24), DynamicIcon(25, a path string),
//! DynamicTooltip_lang(26-34), ExtendedUI(35, a token string), ExtendedUIStateVariable[3](36-38)`.
//!
//! Those names are the **binary's**, confirmed 2026-08-25: the loader's own dimension asserts
//! (`0x553a9e cmp eax,0x27`, `0x553ad6 cmp eax,0x9c`) fix 39 columns of 156 bytes, and
//! `GetWorldStateUIInfo 0x4c5a70` reads exactly `+0x0c` Icon, `+0x10` Text, `+0x34` Tooltip,
//! `+0x5c` StateVariable, `+0x64` DynamicIcon, `+0x68` DynamicTooltip, `+0x8c` ExtendedUI,
//! `+0x90/94/98` the three extended ids. **Field 22 (`+0x58`) is written by the loader and read by
//! nothing** — none of the table's four consumers touch it — so it is not carried here; its
//! shipped values are `0` on the three Eastern Plaguelands rows and `-1` on the other seventeen.
//!
//! The shipped rows read exactly as that naming predicts: the Eastern Plaguelands pair carries
//! `Icon = Interface\WorldStateFrame\{Alliance,Horde}Tower` with `StateVariable = "Towers
//! Controlled: %2327w"` — the same world-state macro grammar the NPC-text expander already
//! speaks, over the two ids vmangos's `OutdoorPvPEP` pushes; Warsong Gulch's two carry a
//! `DynamicIcon` of the *enemy* flag with `WorldState` 2338/2339, the "flag has been picked up"
//! keys; and Alterac Valley's `Type = 2` rows are the scoreboard's column headers, not an
//! always-up readout.
//!
//! What the client *does* with these rows — which it admits, and how [`WorldStateUiRow::text`]
//! becomes a displayed string — is [`crate::world_state_ui`](benilla_app)'s, not this module's.

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const WORLD_STATE_UI: &str = "DBFilesClient\\WorldStateUI.dbc";

/// One `WorldStateUI.dbc` row.
#[derive(Clone, Debug)]
pub struct WorldStateUiRow {
    /// `Map.dbc` id this row belongs to — `0`/`1` for the two outdoor-PvP continents, otherwise a
    /// battleground map (30 Alterac Valley, 489 Warsong Gulch, 529 Arathi Basin).
    pub map_id: u32,
    /// `AreaTable.dbc` id narrowing the row within its map, or `0` for the whole map. Only the
    /// outdoor rows use it: `139` (Eastern Plaguelands) and `1377` (Silithus).
    pub area_id: u32,
    /// The static icon's texture path (`Interface\WorldStateFrame\AllianceTower`), empty when the
    /// row draws no icon.
    pub icon: String,
    /// The label, as authored — a format string in the `%<id>w` world-state grammar
    /// (`"Towers Controlled: %2327w"`, `"Bases: %1779w  Resources: %1776w/%1780w"`), or plain text
    /// on the scoreboard-column rows (`"Graveyards Assaulted"`). This is the **only** column the
    /// client expands; every other string below is handed to Lua verbatim.
    pub text: String,
    /// The tooltip line (`"Alliance Towers Controlled"`).
    pub tooltip: String,
    /// `StateVariable` (+0x5c) — the world-state id whose *value* the client returns as the row's
    /// `uiState`, or `0` for "no state", which the binding answers as the constant `1.0`. Set on
    /// three rows: `2338`/`2339` on the Warsong Gulch pair (the "enemy flag picked up" keys that
    /// swap in [`Self::dynamic_icon`]) and `2426` on the Eastern Plaguelands progress row.
    pub state_variable: u32,
    /// `Type` (+0x60) — which list the row belongs to, and the builder's third gate:
    ///
    /// - **0** — always-up, unconditional. The four Warsong Gulch / Arathi Basin status rows.
    /// - **1** — always-up, but only while the player has joined a **zone-dependent defense**
    ///   chat channel ([`crate::chat_channel_flags`]' `ZONE_DEP | DEFENSE`, which is row 22 alone
    ///   in the shipped table). The five world-PvP rows: Eastern Plaguelands' three and
    ///   Silithus's two.
    /// - **2** — a battleground **scoreboard column**, which never enters the always-up list at
    ///   all; it is built by a different handler off the same table.
    pub ui_type: u32,
    /// The alternate icon shown while [`Self::state_variable`] is live — the enemy flag, on the two
    /// Warsong Gulch rows that have one. Empty elsewhere.
    pub dynamic_icon: String,
    /// The tooltip that goes with [`Self::dynamic_icon`] (`"Horde flag has been picked up"`).
    pub dynamic_tooltip: String,
    /// A token naming an extra UI widget the row drives — `"CAPTUREPOINT"`, on exactly one row
    /// (the Eastern Plaguelands progress bar). Empty elsewhere.
    pub extended_ui: String,
    /// The world-state **ids** that widget reads — the binding answers their resolved *values*.
    /// Nonzero on exactly one row: id 138 carries `(2427, 2428, 0)`, so the whole extended-UI
    /// mechanism in 1.12 exists for Eastern Plaguelands' capture-point progress bar alone.
    pub extended_ui_state: [u32; 3],
}

/// `WorldStateUI.dbc` rows in **file order**, with their ids.
pub struct WorldStateUiCatalog {
    rows: Vec<(u32, WorldStateUiRow)>,
}

impl WorldStateUiCatalog {
    /// A catalog over rows given directly — for a caller that needs a table of exactly known
    /// shape (the consumer's gate tests build one-row ones, so that "no init yet ⇒ empty readout"
    /// is asserted rather than hoped for).
    pub fn from_rows(rows: Vec<(u32, WorldStateUiRow)>) -> Self {
        Self { rows }
    }

    /// Every row in file order. Order is kept because a UI list built from this table is indexed
    /// by position, and the table's own order is the only one that is not an invention.
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

/// 39 fields; the three loc blocks are nine dwords each (see the module doc for how their bounds
/// were proven rather than assumed).
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
    // +0x58: the loader writes it, no consumer reads it. Named for what is known about it.
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

    /// The real 5875 table proves the layout — and each assertion below could have failed if a
    /// field boundary were off by one, because every one of them is a *string* landing where a
    /// number would garble it, or a number landing in a range only that column occupies.
    /// Skips without client data.
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

        // ── The Eastern Plaguelands pair: report B190's own subject.
        let alliance = by_id(136);
        assert_eq!((alliance.map_id, alliance.area_id), (0, 139));
        assert_eq!(alliance.icon, "Interface\\WorldStateFrame\\AllianceTower");
        assert_eq!(alliance.text, "Towers Controlled: %2327w");
        assert_eq!(alliance.tooltip, "Alliance Towers Controlled");
        assert_eq!(alliance.ui_type, 1);
        let horde = by_id(137);
        assert_eq!(horde.icon, "Interface\\WorldStateFrame\\HordeTower");
        assert_eq!(horde.text, "Towers Controlled: %2328w");
        // Type 1 — the world-PvP kind, gated on the zone-defense channel by its consumer.
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

        // ── An Alterac Valley scoreboard column: type 2, no icon path of the WorldStateFrame
        // kind, plain text rather than a macro.
        let graveyards = by_id(63);
        assert_eq!(graveyards.map_id, 30);
        assert_eq!(graveyards.ui_type, 2);
        assert_eq!(graveyards.text, "Graveyards Assaulted");
        assert!(!graveyards.text.contains('%'));

        // ── Whole-table shape, each number a fact that could have come out otherwise.
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
