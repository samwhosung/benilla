//! The zone-text family, `GetZoneText`, `GetRealZoneText`, `GetSubZoneText`,
//! `GetMinimapZoneText` and `GetZonePVPInfo`: readers of the zone caches the host pushes on an
//! area change, before it fires `MINIMAP_ZONE_CHANGED` or the `ZONE_CHANGED` family. The caches
//! live in the [`Model`], out of an addon's reach, as the reference's live in the client's BSS.
//! `GetZonePVPInfo` reads its zone's rows off the commit and, at the call, the realm's PvP flag
//! and the active player's faction template, which the host pushes as they change.

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// The zone caches, as the host resolves them from the area tables.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ZoneTexts {
    /// `GetZoneText`: the zone's name, replaced by the WMO interior's name indoors.
    pub zone: String,
    /// `GetRealZoneText`: the zone's name before any WMO override.
    pub real_zone: String,
    /// `GetSubZoneText`: the leaf area's name, `""` when the leaf is the zone, and indoors.
    pub subzone: String,
    /// `GetMinimapZoneText`: the subzone, else the zone.
    pub minimap: String,
    /// The `AreaTable.dbc` row of the zone the commit stored (`0xb4e314`), as `GetZonePVPInfo`
    /// reads it; `None` for an id with no row (`0x48d5b4`).
    pub pvp_zone: Option<ZonePvp>,
    /// `GetZonePVPInfo`'s isArena: the leaf area's flag `0x80` (`0xb4e318`, `0x48d575`), the
    /// free-for-all pit.
    pub is_arena: bool,
}

/// What `GetZonePVPInfo` reads of the zone's `AreaTable.dbc` row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ZonePvp {
    /// Flags `0x10`, the six capitals: the gate opens there on a PvE realm (`0x48d5cc`).
    pub capital: bool,
    /// `FactionGroupMask` (`+0x50`), tested against the template's masks (`0x48d604`-`0x48d613`).
    pub faction_group_mask: u32,
    /// `FactionGroup.dbc`'s name for the first row whose bit the mask holds and whose name is not
    /// empty, else `""` (`0x48d633`-`0x48d684`).
    pub faction_name: String,
}

/// `GetZonePVPInfo`'s first two values: nil with no zone row (`0x48d5b4`), no active player
/// (`0x48d5bc`), the gate closed, neither the realm flag `0x88272c` nor the capital bit
/// (`0x48d5c2`-`0x48d5d0`), or no template row (`0x48d5e1`-`0x48d5fe`). Open, `"friendly"` when
/// the template's friend mask meets the zone's, else `"hostile"` when its enemy mask does, else
/// `"contested"` (`0x48d607`-`0x48d61d`), with the zone's faction name.
fn zone_pvp_type(
    zone: Option<&ZonePvp>,
    realm_pvp: bool,
    template: Option<(u32, u32)>,
) -> Option<(&'static str, &str)> {
    let zone = zone.filter(|z| realm_pvp || z.capital)?;
    let (friend, enemy) = template?;
    let mask = zone.faction_group_mask;
    let ty = if mask & friend != 0 {
        "friendly"
    } else if mask & enemy != 0 {
        "hostile"
    } else {
        "contested"
    };
    Some((ty, &zone.faction_name))
}

impl super::UiScript {
    /// Push the zone caches; the caller fires the zone events after.
    pub fn set_zone_texts(&mut self, texts: ZoneTexts) {
        self.model_mut().zone = texts;
    }

    /// Push the realm's PvP flag, `0x88272c`, which `GetZonePVPInfo` reads at the call.
    pub fn set_realm_pvp(&mut self, on: bool) {
        self.model_mut().realm_pvp = on;
    }

    /// Push the active player's faction template masks, `(friend, enemy)`, `None` with no active
    /// player or no template row: what `GetZonePVPInfo` reads at the call (`0x48d5d6`-`0x48d613`).
    pub fn set_player_faction_masks(&mut self, masks: Option<(u32, u32)>) {
        self.model_mut().player_faction_masks = masks;
    }
}

/// Register the five zone getters.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    // The four text getters (`0x48a0a0`, `0x48a0c0`, `0x48a0e0`, `0x48a100`) each push one string,
    // their cache slot or the shared empty string `0x882748` when the slot is unset, never nil.
    let text = |pick: fn(&ZoneTexts) -> &str| {
        move |lua: &Lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            lua.create_string(pick(&model.zone))
        }
    };
    g.set("GetZoneText", lua.create_function(text(|z| &z.zone))?)?;
    g.set(
        "GetRealZoneText",
        lua.create_function(text(|z| &z.real_zone))?,
    )?;
    g.set("GetSubZoneText", lua.create_function(text(|z| &z.subzone))?)?;
    g.set(
        "GetMinimapZoneText",
        lua.create_function(text(|z| &z.minimap))?,
    )?;
    // GetZonePVPInfo() (`0x48d540`): always three values, `pvpType, factionName, isArena`; the
    // type is never `"arena"`, and isArena is 1 or nil, never a boolean.
    g.set(
        "GetZonePVPInfo",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let z = &model.zone;
            let pvp = zone_pvp_type(
                z.pvp_zone.as_ref(),
                model.realm_pvp,
                model.player_faction_masks,
            );
            let string = |s: Option<&str>| match s {
                Some(s) => lua.create_string(s).map(Value::String),
                None => Ok(Value::Nil),
            };
            Ok(MultiValue::from_vec(vec![
                string(pvp.map(|(ty, _)| ty))?,
                string(pvp.map(|(_, faction)| faction))?,
                if z.is_arena {
                    Value::Number(1.0)
                } else {
                    Value::Nil
                },
            ]))
        })?,
    )?;
    Ok(())
}
