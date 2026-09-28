//! The zone-text family, `GetZoneText`, `GetRealZoneText`, `GetSubZoneText`,
//! `GetMinimapZoneText` and `GetZonePVPInfo`: readers of the zone caches the host pushes on an
//! area change, before it fires `MINIMAP_ZONE_CHANGED` or the `ZONE_CHANGED` family. The caches
//! live in the [`Model`], out of an addon's reach, as the reference's live in the client's BSS.

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
    /// `GetZonePVPInfo`'s type, `"friendly"`, `"hostile"` or `"contested"`; `None` when the
    /// player, the zone or its faction template is missing.
    pub pvp_type: Option<String>,
    /// `GetZonePVPInfo`'s faction, `FactionGroup.dbc`'s name for the zone's mask bit; `None` for
    /// none.
    pub pvp_faction: Option<String>,
    /// `GetZonePVPInfo`'s isArena: the leaf area's flag `0x80`, the free-for-all pit.
    pub is_arena: bool,
}

impl super::UiScript {
    /// Push the zone caches; the caller fires the zone events after.
    pub fn set_zone_texts(&mut self, texts: ZoneTexts) {
        self.model_mut().zone = texts;
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
            let string = |s: &Option<String>| match s {
                Some(s) => lua.create_string(s).map(Value::String),
                None => Ok(Value::Nil),
            };
            Ok(MultiValue::from_vec(vec![
                string(&z.pvp_type)?,
                string(&z.pvp_faction)?,
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
