//! The `StatusBar` methods (`CSimpleStatusBar`, factory `0x6eef20`, `LoadXML` `0x782ef0`,
//! orientations `0x811b00`). The fill is a left-anchored crop, the art sliced and never squeezed:
//! `SetValue` sets the UV's `u1` to the fill fraction and the right edge with it (`0x770410`),
//! applied at extract.
//! Only StatusBar frames answer these, so a duck-typing addon sees nil on every other kind.

use mlua::{Lua, Table, Value};

use super::binding_abi;
use super::object::{draw_layer_from_str, frame_handle_of};
use super::region::{install_into_slot, load_path_leg, region_wrapper, slot_arg, SlotArg};
use super::{event, Model};
use crate::order::DrawLayer;
use crate::widget::{KindState, RegionHandle, StatusBarState};

/// Registry key of the StatusBar method table.
pub(super) const REG_STATUSBAR_METHODS: &str = "__benilla_statusbar_methods";

/// Run `f` over a frame's StatusBar state; errors on any other receiver, since the method table
/// is a plain Lua value a caller can misapply.
fn with_bar<T>(
    lua: &Lua,
    this: &Table,
    f: impl FnOnce(&mut StatusBarState) -> T,
) -> mlua::Result<T> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let frame = model
        .arena
        .frame_mut(h)
        .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
    match &mut frame.kind_state {
        KindState::StatusBar(sb) => Ok(f(sb)),
        _ => Err(mlua::Error::runtime("not a StatusBar")),
    }
}

/// Store the value clamped to the range, `Some` when it changed (the caller fires
/// `OnValueChanged`); a degenerate range pins to `min`.
fn store_value(sb: &mut StatusBarState, v: f32) -> Option<f32> {
    let clamped = v.clamp(sb.min, sb.max.max(sb.min));
    (clamped != sb.value).then(|| {
        sb.value = clamped;
        clamped
    })
}

/// Install `incoming` as the bar texture, or clear it, through `0x7831f0`: the same texture is a
/// no-op, the displaced one is deleted, the incoming one is linked in on `layer` shown (`0x78321b`)
/// and its texcoord-modifies-rect flag is set (`0x783220`).
fn install_bar_model(
    model: &mut Model,
    h: crate::widget::FrameHandle,
    incoming: Option<RegionHandle>,
    layer: DrawLayer,
) -> mlua::Result<()> {
    if !matches!(
        model.arena.frame(h).map(|f| &f.kind_state),
        Some(KindState::StatusBar(_))
    ) {
        return Err(mlua::Error::runtime("not a StatusBar"));
    }
    let changed = install_into_slot(
        model,
        h,
        incoming,
        layer,
        true,
        false,
        |ks| match ks {
            KindState::StatusBar(sb) => sb.bar,
            _ => None,
        },
        |ks, rh| {
            if let KindState::StatusBar(sb) = ks {
                sb.bar = rh;
            }
        },
    )?;
    if let (true, Some(rh)) = (changed, incoming) {
        model
            .region_data
            .entry(rh)
            .or_default()
            .tex_coord_modifies_rect = true;
    }
    Ok(())
}

fn install_bar(
    lua: &Lua,
    this: &Table,
    incoming: Option<RegionHandle>,
    layer: DrawLayer,
) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    install_bar_model(&mut model, h, incoming, layer)
}

/// `<BarTexture>` (`0x782f63`): the texture the texture adder built, installed as an object on the
/// element's `drawLayer`, ARTWORK without one.
pub(super) fn install_loaded(
    model: &mut Model,
    h: crate::widget::FrameHandle,
    rh: RegionHandle,
    layer: Option<DrawLayer>,
) -> mlua::Result<()> {
    install_bar_model(model, h, Some(rh), layer.unwrap_or(DrawLayer::Artwork))
}

/// `SetStatusBarTexture`'s path leg (`0x783140`, [`load_path_leg`]): a bar that exists has its file
/// swapped and nothing else, its layer included; with none, a texture that loads is installed on
/// `layer` (`0x7831cb`).
fn set_bar_path(lua: &Lua, this: &Table, path: &str, layer: DrawLayer) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let existing = match &model
        .arena
        .frame(h)
        .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?
        .kind_state
    {
        KindState::StatusBar(sb) => sb.bar,
        _ => return Err(mlua::Error::runtime("not a StatusBar")),
    };
    if let Some(rh) = load_path_leg(&mut model, h, existing, layer, path)? {
        // `SetAllPoints` onto the bar (`0x7831bf`).
        super::region::implicit_creation_anchor(&mut model, rh);
        install_bar_model(&mut model, h, Some(rh), layer)?;
    }
    Ok(())
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    m.set(
        "SetMinMaxValues",
        lua.create_function(|lua, (this, min, max): (Table, Value, Value)| {
            // Both `lua_isnumber` (`0x78f542`, `0x78f553`) else
            // `Usage: %s:SetMinMaxValues(min, max)` (`0x87b160`).
            with_bar(lua, &this, |_| ())?;
            let line = "SetMinMaxValues(min, max)";
            let min = binding_abi::method_number(lua, &this, min, line)? as f32;
            let max = binding_abi::method_number(lua, &this, max, line)? as f32;
            // A reversed pair is swapped, as `LoadXML` does; the held value re-clamps, and a move
            // fires `OnValueChanged`.
            let changed = with_bar(lua, &this, |sb| {
                (sb.min, sb.max) = if min <= max { (min, max) } else { (max, min) };
                store_value(sb, sb.value)
            })?;
            fire_value_changed(lua, &this, changed)
        })?,
    )?;
    m.set(
        "GetMinMaxValues",
        lua.create_function(|lua, this: Table| with_bar(lua, &this, |sb| (sb.min, sb.max)))?,
    )?;
    m.set(
        "SetValue",
        lua.create_function(|lua, (this, v): (Table, Value)| {
            // `lua_isnumber` (`0x78f720`) else `Usage: %s:SetValue(value)` (`0x87b184`).
            with_bar(lua, &this, |_| ())?;
            let v = binding_abi::method_number(lua, &this, v, "SetValue(value)")? as f32;
            let changed = with_bar(lua, &this, |sb| store_value(sb, v))?;
            fire_value_changed(lua, &this, changed)
        })?,
    )?;
    m.set(
        "GetValue",
        lua.create_function(|lua, this: Table| with_bar(lua, &this, |sb| sb.value))?,
    )?;
    m.set(
        "SetOrientation",
        lua.create_function(|lua, (this, o): (Table, Value)| {
            with_bar(lua, &this, |_| ())?;
            let vertical = binding_abi::orientation_arg(lua, &this, &o)?;
            with_bar(lua, &this, |sb| sb.vertical = vertical)
        })?,
    )?;
    m.set(
        "GetOrientation",
        lua.create_function(|lua, this: Table| {
            let v = with_bar(lua, &this, |sb| sb.vertical)?;
            Ok(if v { "VERTICAL" } else { "HORIZONTAL" }.to_string())
        })?,
    )?;

    // `SetStatusBarTexture(texture or "texture" or nil [, "layer"])` (`0x78f870`): a Texture object
    // is installed as the bar, linked on `layer` (ARTWORK without one, `0x78f902`) and displacing
    // the old bar (`0x7831f0`); a string or a number is the path of the bar (`0x783140`), and nil
    // clears it (`0x78fa6d`). There is no colour form. The object leg's raises print
    // `SetCheckedTexture`, the client's copy-pasted format strings (`0x87b1e8`, `0x87b224`).
    m.set(
        "SetStatusBarTexture",
        lua.create_function(|lua, (this, args): (Table, mlua::MultiValue)| {
            // `lua_isstring` on the layer (`0x78f909`): a number takes the name parse too, and
            // finds no layer.
            let layer = match args.get(1) {
                Some(v @ (Value::String(_) | Value::Number(_) | Value::Integer(_))) => lua
                    .coerce_string(v.clone())?
                    .and_then(|l| draw_layer_from_str(&l.to_string_lossy())),
                _ => None,
            }
            .unwrap_or(DrawLayer::Artwork);
            match slot_arg(
                lua,
                &this,
                &args,
                r#"SetStatusBarTexture(texture or "texture" or nil [, "layer"])"#,
                "SetCheckedTexture",
                true,
            )? {
                SlotArg::Object(rh) => install_bar(lua, &this, Some(rh), layer),
                SlotArg::Clear => install_bar(lua, &this, None, DrawLayer::Artwork),
                SlotArg::Path(path) => set_bar_path(lua, &this, &path, layer),
            }
        })?,
    )?;
    m.set(
        "GetStatusBarTexture",
        lua.create_function(|lua, this: Table| {
            let bar = with_bar(lua, &this, |sb| sb.bar)?;
            let id = {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                bar.map(|rh| model.region_id(rh))
            };
            match id {
                Some(id) => Ok(Value::Table(region_wrapper(lua, id)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;
    // `0x78fc20` reads r, g, b by a bare `lua_tonumber`, so nil, a table or a non-numeric string
    // is 0.0 and the call never raises (a numeric string also reads 0.0 here). Stock hands colour
    // setters nils: `QuestLogFrame.lua:337` passes three to `SetVertexColor`.
    m.set(
        "SetStatusBarColor",
        lua.create_function(
            |lua, (this, r, g, b, a): (Table, Value, Value, Value, Value)| {
                let c = crate::script::object::colour_args(lua, &r, &g, &b, &a);
                // `0x78fc20` hands the colour to the bar (`0x782dd0`), which sets its vertex colour
                // (`0x77f750`, collapsing a gradient) only if a bar exists and keeps nothing
                // otherwise.
                let Some(rh) = with_bar(lua, &this, |sb| sb.bar)? else {
                    return Ok(());
                };
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model
                    .region_data
                    .entry(rh)
                    .or_default()
                    .set_vertex_color(Some(c));
                Ok(())
            },
        )?,
    )?;
    m.set(
        "GetStatusBarColor",
        lua.create_function(|lua, this: Table| {
            let bar = with_bar(lua, &this, |sb| sb.bar)?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            // Each byte times the float at `0x81c6b4` (`0x78fab0`).
            let c = bar
                .and_then(|rh| model.region_data.get(&rh))
                .map_or(crate::color::Rgba8::WHITE, |d| d.vertex_color_or_white());
            Ok(crate::script::object::colour_values(c))
        })?,
    )?;

    lua.set_named_registry_value(REG_STATUSBAR_METHODS, m)?;
    Ok(())
}

/// Fire `OnValueChanged` (the StatusBar's script slot, `+0x32c`) for a changed value, outside any
/// model borrow.
fn fire_value_changed(lua: &Lua, this: &Table, changed: Option<f32>) -> mlua::Result<()> {
    let Some(value) = changed else { return Ok(()) };
    let id = {
        let h = frame_handle_of(lua, this)?;
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        model.frame_id(h)
    };
    if let Err(e) = event::fire_widget_handler(
        lua,
        id,
        "OnValueChanged",
        vec![Value::Number(f64::from(value))],
    ) {
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .record_lua_error(&e);
    }
    Ok(())
}

/// `OnValueChanged` for a bar the engine moved, the nameplate health bars: the reference re-sets
/// them from a GUID-watch callback (`0x7cc570`, registered at `0x467e70`) on every server update,
/// and that `SetValue` fires the script as any other does.
pub(super) fn fire_engine_value_changed(lua: &Lua, bar: crate::widget::FrameHandle, value: f32) {
    let id = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        model.frame_id(bar)
    };
    if let Err(e) = event::fire_widget_handler(
        lua,
        id,
        "OnValueChanged",
        vec![Value::Number(f64::from(value))],
    ) {
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .record_lua_error(&e);
    }
}
