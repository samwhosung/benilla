//! The `Minimap` methods: zoom, the mask, the ping and the model attributes. The reference has six
//! levels (`0x6da9a0`), a live index per mode (`0x86f698` outdoor, `0x86f69c` indoor) and the
//! `minimapZoom`/`minimapInsideZoom` CVars (default `"3"`) that re-seed it (`0x6d9008`); the app
//! maps the index to a radius (`0x6da9b0`) and holds the ping as a world point.

use mlua::{Lua, Table, Value};

use super::binding_abi::{lua_number_text, number_arg};
use super::object::{decode_id, frame_handle_of};
use super::Model;
use crate::widget::{FrameHandle, FrameKind, KindState, MINIMAP_ZOOM_LEVELS};

/// Registry key of the Minimap method table.
pub(super) const REG_MINIMAP_METHODS: &str = "__benilla_minimap_methods";

/// The member prologue the setters share: `this` must be a table (`0x847ef8`), carry a
/// framescript object (`0x847ec0`) and be a Minimap, the `IsA` test against the Minimap class
/// token `0xbc8290` (`0x4ee000`, else `0x847e98`).
fn this_minimap(lua: &Lua, this: &Value) -> mlua::Result<FrameHandle> {
    let Value::Table(t) = this else {
        return Err(mlua::Error::runtime(
            "Attempt to find 'this' in non-table object (used '.' instead of ':' ?)",
        ));
    };
    if decode_id(t).is_err() {
        return Err(mlua::Error::runtime(
            "Attempt to find 'this' in non-framescript object",
        ));
    }
    let wrong = || mlua::Error::runtime("Wrong object type for member function");
    let h = frame_handle_of(lua, t).map_err(|_| wrong())?;
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    match model.arena.frame(h).map(|f| f.kind) {
        Some(FrameKind::Minimap) => Ok(h),
        _ => Err(wrong()),
    }
}

/// The frame's name for a usage or load error's `%s`, `<unnamed>` without one.
fn frame_name(lua: &Lua, h: FrameHandle) -> String {
    lua.app_data_ref::<Model>()
        .expect("model app_data")
        .arena
        .frame(h)
        .and_then(|f| f.name.clone())
        .unwrap_or_else(|| "<unnamed>".to_string())
}

/// The model half of `CMinimap::LoadXML` (`0x4ee2b0`): `minimapArrowModel` to engine children 1
/// to 8 (`0x4ee170`), `minimapPlayerModel` to child 9 alone (`0x4ee260`). Runs before the
/// `<Frames>` descent, as the reference chains to `CSimpleFrame::LoadXML` (`0x76a2f0`) after.
pub(crate) fn apply_model_attrs(
    lua: &Lua,
    this: &Table,
    arrow: &str,
    player: &str,
) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let Some(children) = model.arena.frame(h).map(|f| f.children.clone()) else {
        return Ok(());
    };
    for (i, child) in children
        .into_iter()
        .take(crate::widget::MINIMAP_ENGINE_CHILDREN)
        .enumerate()
    {
        let path = if i + 1 == crate::widget::MINIMAP_ENGINE_CHILDREN {
            player
        } else {
            arrow
        };
        if let Some(frame) = model.arena.frame_mut(child) {
            if let KindState::Model(state) = &mut frame.kind_state {
                state.path = Some(path.to_string());
            }
        }
    }
    Ok(())
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    m.set(
        "GetZoom",
        // The indoor index while inside a WMO, else the outdoor one (`0x6da980`); no `this`.
        lua.create_function(|lua, _: mlua::MultiValue| {
            Ok(lua
                .app_data_ref::<Model>()
                .expect("model app_data")
                .minimap
                .active_zoom())
        })?,
    )?;
    m.set(
        "SetZoom",
        lua.create_function(|lua, (_this, zoom): (Value, Value)| {
            // No `this` lookup; argument 2 passes `lua_isnumber` (`0x4eec68`) or raises (`0x4eec8d`).
            let level = number_arg(lua, zoom, "Usage: SetZoom(level)")?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if let Some(cvar) = model.minimap.set_zoom_index(level) {
                // The mode's CVar takes the new level, which is what survives a restart.
                let text = model.minimap.active_zoom().to_string();
                super::cvars::set_from_engine(&mut model, cvar, text);
            }
            Ok(())
        })?,
    )?;
    m.set(
        // The disc's mask art, write-only as in the reference (`0x4ee4a0`). The receiver is checked
        // first, then `lua_isstring(2)` (`0x4ee532`): anything but a string or a number raises
        // `Usage: %s:SetMaskTexture("file")` (`0x84c7fc`), naming the frame or `<unnamed>`. An
        // empty path restores the default circle rather than unmasking the map; what the
        // reference does with one is untraced.
        "SetMaskTexture",
        lua.create_function(|lua, (this, path): (Value, Value)| {
            let h = this_minimap(lua, &this)?;
            let path = match path {
                Value::String(s) => s.to_string_lossy(),
                Value::Integer(i) => lua_number_text(i as f64),
                Value::Number(n) => lua_number_text(n),
                _ => {
                    return Err(mlua::Error::runtime(format!(
                        "Usage: {}:SetMaskTexture(\"file\")",
                        frame_name(lua, h)
                    )));
                }
            };
            let path = Some(path).filter(|p| !p.is_empty());
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .minimap
                .mask_texture = path;
            Ok(())
        })?,
    )?;
    m.set(
        "GetZoomLevels",
        // The reference's constant (`0x6da9a0`), with no `this` lookup (`0x4eebe0`).
        lua.create_function(|_, _: mlua::MultiValue| Ok(MINIMAP_ZOOM_LEVELS))?,
    )?;
    m.set(
        "PingLocation",
        // Centre-relative offsets in the minimap's own units, x right and y up, which the
        // reference reads against the widget's own size (`0x4eeca0`); parked in screen units, its
        // effective scale applied, for the app to resolve in the frame it draws the map. One
        // pending click, not one per widget.
        lua.create_function(|lua, (this, x, y): (Value, f32, f32)| {
            let h = this_minimap(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let s = super::object::eff_scale(&model, h);
            model.minimap_ping_request = Some((x * s, y * s));
            Ok(())
        })?,
    )?;
    m.set(
        "GetPingPosition",
        // Fractions of the widget side from its centre (`MINIMAP_PING`'s arg2/arg3 space), which
        // the app recomputes from the world point each frame. Always two numbers (`0x4eefd0`): the
        // stock `Minimap_OnUpdate` multiplies them for 5 s, so a nil would error every frame. No
        // `this` lookup.
        lua.create_function(|lua, _: mlua::MultiValue| {
            let (x, y) = lua
                .app_data_ref::<Model>()
                .expect("model app_data")
                .minimap_ping;
            Ok((x, y))
        })?,
    )?;
    lua.set_named_registry_value(REG_MINIMAP_METHODS, m)?;
    Ok(())
}
