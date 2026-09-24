//! The font block: the ten font methods a text-bearing widget's own method table declares,
//! implemented once over the [`RegionData`](super::RegionData) its glyphs paint from.
//!
//! The 1.12 client has no `FontInstance` class: each text type re-declares the font names in its
//! own flat method table, and each binding is a shim that tail-calls one shared implementation.
//! This module is that shared layer, installed by the FontString, EditBox, MessageFrame and
//! ScrollingMessageFrame tables; the `<Font>` object and `SimpleHTML` write their own records and
//! share only [`set_font_args`].
//!
//! `EditBox`'s table (`0x87bb68`, 48 entries) opens with these ten, then `Set/GetSpacing` and the
//! four justify methods. Its shims (`0x797090` on) leave `eax` alone, so each returns what the
//! shared implementation pushes, and act on `[this+0x324]`, the box's implicit FontString
//! ([`EditBoxState::text_region`](crate::widget::EditBoxState::text_region)). The justify four are
//! installed by `script::editbox::methods`. `SetSpacing`/`GetSpacing` (`0x79fb40`/`0x79fbe0`) are
//! not installed: no line spacing is modelled, so a call raises.

use mlua::{Lua, Table, Value};

use super::object::as_f32;
use super::{FontShadow, Model, Outline};
use crate::widget::RegionHandle;

/// How a widget's method table finds the region its glyphs paint from: a `FontString` is one, an
/// `EditBox` creates its own on demand. An error means the wrong receiver.
pub(super) type ResolveRegion = fn(&Lua, &Table) -> mlua::Result<RegionHandle>;

/// `SetFont`'s shared argument gate (`0x79f210`, reached from Font `0x7a0270`, FontString
/// `0x79d4f0` and EditBox `0x797210`): arg 2 must pass `lua_isstring` and arg 3 `lua_isnumber`,
/// both coercing, else it raises `Usage: %s:SetFont("font", fontHeight [, flags])` (`0x87c69c`).
/// An empty path is not an argument error but a failed load, which the caller answers with nil.
pub(super) fn set_font_args(
    file: &Value,
    height: &Value,
    widget: &str,
) -> mlua::Result<(String, f32)> {
    let usage = || {
        mlua::Error::runtime(format!(
            "Usage: <{widget}>:SetFont(\"font\", fontHeight [, flags])"
        ))
    };
    let path = match file {
        Value::String(s) => s.to_str()?.to_string(),
        Value::Number(_) | Value::Integer(_) => as_f32(file).to_string(),
        _ => return Err(usage()),
    };
    let height = match height {
        Value::Number(_) | Value::Integer(_) => as_f32(height),
        Value::String(s) => s.to_str()?.parse::<f32>().map_err(|_| usage())?,
        _ => return Err(usage()),
    };
    Ok((path, height))
}

/// Install the ten font-block methods onto `m`, reading `this` through `resolve`; only for a
/// widget whose reference method table carries all ten.
pub(super) fn install(
    lua: &Lua,
    m: &Table,
    resolve: ResolveRegion,
    widget: &'static str,
) -> mlua::Result<()> {
    // ── the font object ──
    // SetFontObject(font | "font" | nil) → nothing (`0x79ef10`, usage string `0x87c5cc`). Any
    // other argument or an unknown name raises: every rejection is `luaL_error` (`0x6f4940`).
    m.set(
        "SetFontObject",
        lua.create_function(move |lua, (this, font): (Table, Value)| {
            let name = super::font::resolve("SetFontObject", &font)?;
            let rh = resolve(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            // nil severs the link and keeps the paint; the reference only nulls the parent.
            let Some(name) = name else {
                model.region_data.entry(rh).or_default().font_object = None;
                return Ok(());
            };
            let Some(fo) = model.font_object(&name).cloned() else {
                return Err(mlua::Error::runtime(format!(
                    "SetFontObject: no font object named '{name}' is registered"
                )));
            };
            let d = model.region_data.entry(rh).or_default();
            d.font_object = Some(name);
            // The inherit mask stays: each local setter clears its bit (`FONTINSTANCE+0x2c`) and
            // nothing restores it, so a property set locally survives a later `SetFontObject`.
            super::font::repaint(d, &fo);
            model.touch_measure(rh);
            Ok(())
        })?,
    )?;
    // GetFontObject() → the font object last set, never its name, or nil (`0x79f090`).
    m.set(
        "GetFontObject",
        lua.create_function(move |lua, this: Table| {
            let rh = resolve(lua, &this)?;
            let name = {
                let model = lua.app_data_ref::<Model>().expect("model");
                model
                    .region_data
                    .get(&rh)
                    .and_then(|d| d.font_object.clone())
                    .filter(|n| model.font_object(n).is_some())
            };
            match name {
                Some(n) => Ok(Value::Table(super::font::wrapper(lua, &n)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // ── the face ──
    // SetFont(path, height [, flags]) → the number 1 (`0x79f345`), or nil on a failed load
    // (`0x79f361`), never a boolean. The EditBox shim passes it through (`0x7972b2` leaves `eax`
    // alone), unlike `Button:SetFont` (`0x780880`), which returns nothing.
    m.set(
        "SetFont",
        lua.create_function(
            move |lua, (this, file, height, flags): (Table, Value, Value, Option<String>)| {
                let (path, height) = set_font_args(&file, &height, widget)?;
                let rh = resolve(lua, &this)?;
                let mut model = lua.app_data_mut::<Model>().expect("model");
                // A failed load (`0x5c1ae0` under the `0x44d040` font cache) is the host's probe to
                // judge, as only the store knows the files; with no probe, a non-empty path loads.
                let ok =
                    !path.is_empty() && model.font_probe.as_ref().is_none_or(|probe| probe(&path));
                let d = model.region_data.entry(rh).or_default();
                // Each supplied argument is an explicit set, kept through a later font-object
                // change; the height applies even when the face fails to load.
                if ok {
                    d.font_path = Some(path);
                    d.font_explicit.face = true;
                }
                d.font_height = Some(height);
                d.font_explicit.height = true;
                if let Some(f) = flags {
                    // The Lua spelling (`OUTLINE`, `THICKOUTLINE`), not the XML one, parsed like
                    // `0x6f1a90`: a case-insensitive substring scan, so `THICKOUTLINE` sets both.
                    d.outline = Outline::flags(&f);
                    d.font_explicit.outline = true;
                }
                model.touch_measure(rh);
                Ok(if ok { Value::Number(1.0) } else { Value::Nil })
            },
        )?,
    )?;
    // GetFont() → path, height, flags (`0x79f3b0`, `mov eax,3` at `0x79f407`); flags is "" when
    // none (a zeroed buffer at `0xceea60`), never nil. The height is a number even with no font:
    // `0x7727b0` loads `[esi+0xe4]` unconditionally (`0x79d499` skips its `+0xe0` test). That
    // field has no constructor writer, so an unset height reads 0 here, like the Font object's
    // `+0x48`.
    m.set(
        "GetFont",
        lua.create_function(move |lua, this: Table| {
            let rh = resolve(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            let d = model.region_data.get(&rh);
            let path = match d.and_then(|d| d.font_path.clone()) {
                Some(p) => Value::String(lua.create_string(&p)?),
                None => Value::Nil,
            };
            let height = d.and_then(|d| d.font_height).unwrap_or(0.0);
            let flags = d.map(|d| d.outline).unwrap_or_default().as_str();
            Ok((path, height, flags))
        })?,
    )?;

    // ── the text colour ──
    // SetTextColor(r, g, b [, a]) → nothing (`0x79f4d0`). r, g and b are a bare `lua_tonumber`
    // (`0x79d9c0`), so nil reads 0.0 and the call never raises; alpha defaults to 1.0
    // (`lua_isnumber`-gated). The colour is the region's vertex colour, the `+0xb8` slot
    // `SetVertexColor` writes.
    m.set(
        "SetTextColor",
        lua.create_function(
            move |lua, (this, r, g, b, a): (Table, Value, Value, Value, Option<f32>)| {
                let (r, g, b) = (
                    super::object::as_f32(&r),
                    super::object::as_f32(&g),
                    super::object::as_f32(&b),
                );
                let rh = resolve(lua, &this)?;
                let mut model = lua.app_data_mut::<Model>().expect("model");
                let d = model.region_data.entry(rh).or_default();
                d.vertex_color = Some([r, g, b, a.unwrap_or(1.0)]);
                // Clears the colour inherit bit (`0x79dbd0`): it survives a font-object repaint.
                d.font_explicit.color = true;
                Ok(())
            },
        )?,
    )?;
    // GetTextColor() → r, g, b, a (`0x79f680`); white when never set.
    m.set(
        "GetTextColor",
        lua.create_function(move |lua, this: Table| {
            let rh = resolve(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            let c = model
                .region_data
                .get(&rh)
                .and_then(|d| d.vertex_color)
                .unwrap_or([1.0, 1.0, 1.0, 1.0]);
            Ok((c[0], c[1], c[2], c[3]))
        })?,
    )?;

    // ── the shadow ──
    // `SetShadowColor` `0x79f730`, `GetShadowColor` `0x79f910` (four values, not three: `0x79f9b3`
    // `mov eax,0x4`), `SetShadowOffset` `0x79f9c0`, `GetShadowOffset` `0x79fad0` (two, in UI
    // units). Colour and offset share one inherit slot, so each setter keeps the other half.
    m.set(
        "SetShadowColor",
        lua.create_function(
            // r, g and b are a bare `lua_tonumber` (`0x79dd40`), as in `SetTextColor`.
            move |lua, (this, r, g, b, a): (Table, Value, Value, Value, Option<f32>)| {
                let (r, g, b) = (
                    super::object::as_f32(&r),
                    super::object::as_f32(&g),
                    super::object::as_f32(&b),
                );
                let rh = resolve(lua, &this)?;
                let mut model = lua.app_data_mut::<Model>().expect("model");
                let d = model.region_data.entry(rh).or_default();
                let offset = d.font_shadow.map_or([0.0, 0.0], |s| s.offset);
                d.font_shadow = Some(FontShadow {
                    offset,
                    color: [r, g, b, a.unwrap_or(1.0)],
                });
                d.font_explicit.shadow = true;
                Ok(())
            },
        )?,
    )?;
    m.set(
        "GetShadowColor",
        lua.create_function(move |lua, this: Table| {
            let rh = resolve(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            let c = model
                .region_data
                .get(&rh)
                .and_then(|d| d.font_shadow)
                .map_or([0.0, 0.0, 0.0, 1.0], |s| s.color);
            Ok((c[0], c[1], c[2], c[3]))
        })?,
    )?;
    // SetShadowOffset(x, y): both required, else `Usage: %s:SetShadowOffset(x, y)` (`0x87c6e8`).
    m.set(
        "SetShadowOffset",
        lua.create_function(move |lua, (this, x, y): (Table, f32, f32)| {
            let rh = resolve(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let d = model.region_data.entry(rh).or_default();
            let color = d.font_shadow.map_or([0.0, 0.0, 0.0, 1.0], |s| s.color);
            d.font_shadow = Some(FontShadow {
                offset: [x, y],
                color,
            });
            d.font_explicit.shadow = true;
            Ok(())
        })?,
    )?;
    m.set(
        "GetShadowOffset",
        lua.create_function(move |lua, this: Table| {
            let rh = resolve(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            let o = model
                .region_data
                .get(&rh)
                .and_then(|d| d.font_shadow)
                .map_or([0.0, 0.0], |s| s.offset);
            Ok((o[0], o[1]))
        })?,
    )?;

    Ok(())
}
