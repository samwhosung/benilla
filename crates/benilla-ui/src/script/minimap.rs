//! The `Minimap` methods: zoom, the mask, the ping and the model attributes. The reference has six
//! levels (`0x6da9a0`), a live index per mode (`0x86f698` outdoor, `0x86f69c` indoor) and the
//! `minimapZoom`/`minimapInsideZoom` CVars (default `"3"`) that re-seed it (`0x6d9008`); the app
//! maps the index to a radius (`0x6da9b0`) and holds the ping as a world point.

use mlua::{Lua, Table};

use super::object::frame_handle_of;
use super::Model;
use crate::widget::{KindState, MinimapState, MINIMAP_ZOOM_LEVELS};

/// Registry key of the Minimap method table.
pub(super) const REG_MINIMAP_METHODS: &str = "__benilla_minimap_methods";

/// Run `f` over a frame's Minimap state; errors on any other receiver, since the method table is
/// a plain Lua value a caller can misapply.
fn with_minimap<T>(
    lua: &Lua,
    this: &Table,
    f: impl FnOnce(&mut MinimapState) -> T,
) -> mlua::Result<T> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let frame = model
        .arena
        .frame_mut(h)
        .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
    match &mut frame.kind_state {
        KindState::Minimap(m) => Ok(f(m)),
        _ => Err(mlua::Error::runtime("not a Minimap")),
    }
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
        // The indoor index while inside a WMO, else the outdoor one, as the reference routes.
        lua.create_function(|lua, this: Table| with_minimap(lua, &this, |m| m.active_zoom()))?,
    )?;
    m.set(
        "SetZoom",
        lua.create_function(|lua, (this, zoom): (Table, f64)| {
            // Truncated and clamped to 0..=5 (`0x6daa10`), into the current mode's index.
            let clamped = (zoom.max(0.0) as u8).min(MINIMAP_ZOOM_LEVELS - 1);
            let inside = with_minimap(lua, &this, |m| {
                m.set_active_zoom(clamped);
                m.inside
            })?;
            // The reference also sets the mode's CVar, which is what survives a restart. The borrow
            // above is released first: both reach the same `Model`.
            super::cvars::set_from_engine(
                &mut lua.app_data_mut::<Model>().expect("model app_data"),
                if inside {
                    "minimapInsideZoom"
                } else {
                    "minimapZoom"
                },
                clamped.to_string(),
            );
            Ok(())
        })?,
    )?;
    m.set(
        // The disc's mask art, write-only as in the reference. An absent or empty path restores
        // the default circle rather than unmasking the map; what the reference's `0x4ee4a0` does
        // with one is untraced.
        "SetMaskTexture",
        lua.create_function(|lua, (this, path): (Table, Option<String>)| {
            let path = path.filter(|p| !p.is_empty());
            with_minimap(lua, &this, |m| m.mask_texture = path)
        })?,
    )?;
    m.set(
        "GetZoomLevels",
        lua.create_function(|lua, this: Table| {
            // The reference's constant, with the receiver still validated.
            with_minimap(lua, &this, |_| MINIMAP_ZOOM_LEVELS)
        })?,
    )?;
    m.set(
        "PingLocation",
        // Centre-relative offsets in UI units, x right and y up, parked for the app to resolve in
        // the frame it draws the map; one pending click, not one per widget.
        lua.create_function(|lua, (this, x, y): (Table, f32, f32)| {
            with_minimap(lua, &this, |_| ())?;
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .minimap_ping_request = Some((x, y));
            Ok(())
        })?,
    )?;
    m.set(
        "GetPingPosition",
        // Fractions of the widget side from its centre (`MINIMAP_PING`'s arg2/arg3 space), which
        // the app recomputes from the world point each frame. Always two numbers (`0x4eefd0`): the
        // stock `Minimap_OnUpdate` multiplies them for 5 s, so a nil would error every frame.
        lua.create_function(|lua, this: Table| {
            with_minimap(lua, &this, |_| ())?;
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
