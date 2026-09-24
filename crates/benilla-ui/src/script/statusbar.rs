//! The `StatusBar` method surface — the first *per-kind* widget behavior over the kind tag
//! (`CSimpleStatusBar`, factory `0x6eef20`).
//!
//! Grounded in the reference's StatusBar `LoadXML 0x782ef0`:
//! a reversed `min > max` pair is swapped, `defaultValue` routes through SetValue, orientation is
//! the shared enum HORIZONTAL=0/VERTICAL=1 (`0x811b00`), the bar texture's layer defaults ARTWORK,
//! and the widget adds the `OnValueChanged` script slot (`+0x32c`). The **fill** mechanism is
//! byte-pinned too (`0x770410`): `SetValue` writes the bar region's 4-corner UV block with
//! `u1 = GetValue()` *and* recomputes `right = left + frac·width`, so the fill is a left-anchored
//! **crop** — the art is sliced, never squeezed. Applied at extract (`bar_fill_rect`/`bar_fill_uv`).
//!
//! The methods live in their own registry table, consulted by the frame `__index` dispatcher only
//! for StatusBar frames — so duck-typing addons (`if frame.SetValue then …`) see `nil` on every
//! other kind, exactly as against the client's per-class method sets.

use mlua::{Lua, Table, Value};

use super::object::{draw_layer_from_str, frame_handle_of};
use super::region::region_wrapper;
use super::{event, Model, RegionData};
use crate::order::DrawLayer;
use crate::widget::{KindState, RegionKind, StatusBarState};

/// Registry key of the StatusBar method table (the MAXCSTACK discipline: Lua-side root, named key).
pub(super) const REG_STATUSBAR_METHODS: &str = "__benilla_statusbar_methods";

/// Run `f` over a frame's StatusBar state under one short write borrow. Errors if `this` is not a
/// live StatusBar (unreachable through the kind dispatcher, but the method table is a plain Lua
/// value — a caller can fish it out and misapply it).
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

/// Store a (clamped) value; returns `Some(new_value)` if it actually changed — the caller fires
/// `OnValueChanged`. Degenerate range (`max <= min`) pins to `min` (zero-init members).
fn store_value(sb: &mut StatusBarState, v: f32) -> Option<f32> {
    let clamped = v.clamp(sb.min, sb.max.max(sb.min));
    (clamped != sb.value).then(|| {
        sb.value = clamped;
        clamped
    })
}

/// Get-or-create the bar-fill texture region (`SetStatusBarTexture`/`<BarTexture>`); `layer`
/// re-layers an existing bar. Returns the region's id (for wrapper lookup).
fn ensure_bar(lua: &Lua, this: &Table, layer: Option<DrawLayer>) -> mlua::Result<u32> {
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

    let rh = match existing {
        Some(rh) => {
            if let (Some(l), Some(region)) = (layer, model.arena.region_mut(rh)) {
                region.draw_layer = l;
            }
            rh
        }
        None => {
            let rh = model
                .arena
                .create_region(
                    h,
                    RegionKind::Texture,
                    layer.unwrap_or(DrawLayer::Artwork),
                    0,
                )
                .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
            model.region_data.insert(rh, RegionData::default());
            model.touch_layout(); // a region entered the layout gate's read set (decision 0740)
            if let Some(frame) = model.arena.frame_mut(h) {
                if let KindState::StatusBar(sb) = &mut frame.kind_state {
                    sb.bar = Some(rh);
                }
            }
            rh
        }
    };
    Ok(model.region_id(rh))
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    m.set(
        "SetMinMaxValues",
        lua.create_function(|lua, (this, min, max): (Table, f32, f32)| {
            // A reversed pair is swapped (`LoadXML 0x782ef0` does the same) — one behavior for the
            // XML and API paths. The held value re-clamps into the new range; a move fires
            // OnValueChanged, as a value change the caller didn't make explicitly is still a change.
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
        lua.create_function(|lua, (this, v): (Table, f32)| {
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
        lua.create_function(|lua, (this, o): (Table, String)| {
            let vertical = match o.to_ascii_uppercase().as_str() {
                "HORIZONTAL" => false,
                "VERTICAL" => true,
                _ => {
                    return Err(mlua::Error::runtime(format!(
                        "SetOrientation: unknown orientation '{o}'"
                    )))
                }
            };
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

    // SetStatusBarTexture(path [, drawLayer]) | SetStatusBarTexture(r, g, b [, a]) — the same two
    // forms as a region's SetTexture, targeting the bar region (created on first use).
    m.set(
        "SetStatusBarTexture",
        lua.create_function(
            |lua, (this, a1, a2, a3, a4): (Table, Value, Value, Value, Value)| {
                let layer = match &a2 {
                    Value::String(s) => s.to_str().ok().and_then(|l| draw_layer_from_str(&l)),
                    _ => None,
                };
                let id = ensure_bar(lua, &this, layer)?;
                let rh = {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    *model.id_to_region.get(&id).expect("bar region id")
                };
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                let data = model.region_data.entry(rh).or_default();
                match &a1 {
                    Value::String(s) => {
                        data.texture = Some(s.to_str()?.to_string());
                        data.fill = None;
                    }
                    // A solid bar writes the same slot the path form does — each clears the other.
                    // `SetStatusBarColor` is the TINT (the fill region's vertex colour) and is a
                    // separate slot entirely: a bar can carry art AND a colour, and they multiply.
                    Value::Number(_) | Value::Integer(_) => {
                        data.texture = None;
                        data.fill = Some([
                            num_f32(&a1),
                            num_f32(&a2),
                            num_f32(&a3),
                            match &a4 {
                                Value::Nil => 1.0,
                                v => num_f32(v),
                            },
                        ]);
                    }
                    _ => {}
                }
                Ok(())
            },
        )?,
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
    // **Shape C on r, g, b** (`StatusBar:SetStatusBarColor `0x78fc20``, `2=C 3=C 4=C 5=B`): a bare
    // `lua_tonumber` with no `lua_isnumber` gate, so a nil, a table or a string is **0.0** and the
    // call never raises. Taking them as `f32` made mlua's
    // converter the gate instead — the 2176 class — and the stock
    // `QuestLogFrame.lua:337` idiom hands three nils (`titleButton.r/g/b` are only assigned in
    // `QuestLog_Update`) on any path that selects a quest-log entry before the window has painted.
    m.set(
        "SetStatusBarColor",
        lua.create_function(
            |lua, (this, r, g, b, a): (Table, Value, Value, Value, Option<f32>)| {
                let (r, g, b) = (
                    crate::script::object::as_f32(&r),
                    crate::script::object::as_f32(&g),
                    crate::script::object::as_f32(&b),
                );
                let id = ensure_bar(lua, &this, None)?;
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                let rh = *model.id_to_region.get(&id).expect("bar region id");
                model.region_data.entry(rh).or_default().vertex_color =
                    Some([r, g, b, a.unwrap_or(1.0)]);
                Ok(())
            },
        )?,
    )?;
    m.set(
        "GetStatusBarColor",
        lua.create_function(|lua, this: Table| {
            let bar = with_bar(lua, &this, |sb| sb.bar)?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let c = bar
                .and_then(|rh| model.region_data.get(&rh))
                .and_then(|d| d.vertex_color)
                .unwrap_or([1.0, 1.0, 1.0, 1.0]);
            Ok((c[0], c[1], c[2], c[3]))
        })?,
    )?;

    lua.set_named_registry_value(REG_STATUSBAR_METHODS, m)?;
    Ok(())
}

/// Fire `OnValueChanged(self, value)` if `changed` carries the new value (the StatusBar's own
/// script slot `+0x32c`). Fired outside any model borrow; errors go to [`Model::errors`].
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
            .record_script_error(e.to_string());
    }
    Ok(())
}

/// `OnValueChanged` for a bar the ENGINE moved rather than Lua — the nameplate health bars, whose
/// value the plate driver re-sets from the unit's descriptor each time it changes (decision 2148).
///
/// The reference fires the same script from the same place: the plate's bar is driven by a
/// GUID-watch callback (`0x7cc570`, registered `0x467e70`) that re-reads health and re-sets the bar
/// synchronously on every server update, and `SetValue` is `SetValue`. pfUI hooks this
/// (`nameplates.lua:393` `HookScript(healthbar, "OnValueChanged", …)`) and reads `this:GetParent()`
/// inside the handler, so a driver that moved the value silently would leave it blind.
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
            .record_script_error(e.to_string());
    }
}

/// A Lua number-ish → f32 (nil/other → 0.0), for the color-form arguments.
fn num_f32(v: &Value) -> f32 {
    match v {
        Value::Number(n) => *n as f32,
        Value::Integer(i) => *i as f32,
        _ => 0.0,
    }
}
