//! The font block: the twelve font methods a text-bearing widget's own method table declares,
//! implemented once over the [`RegionData`](super::RegionData) its glyphs paint from.
//!
//! The 1.12 client has no `FontInstance` class: each text type re-declares the font names in its
//! own flat method table, and each binding is a shim that tail-calls one shared implementation.
//! This module is that shared layer, installed by the FontString, EditBox, MessageFrame and
//! ScrollingMessageFrame tables; the `<Font>` object and `SimpleHTML` write their own records and
//! share only the argument gates ([`set_font_args`], [`spacing_arg`]).
//!
//! `EditBox`'s table (`0x87bb68`, 48 entries) opens with these twelve, then the four justify
//! methods. Its shims (`0x797090` on) leave `eax` alone, so each returns what the shared
//! implementation pushes, and act on `[this+0x324]`, the box's font, which here is its implicit
//! FontString ([`EditBoxState::text_region`](crate::widget::EditBoxState::text_region)). The
//! justify four are installed by `script::editbox::methods`.

use mlua::{Lua, Table, Value};

use super::binding_abi;
use super::{FontShadow, Model, Outline};
use crate::widget::RegionHandle;

/// How a widget's method table finds the region its glyphs paint from: a `FontString` is one, an
/// `EditBox` creates its own on demand. An error means the wrong receiver.
pub(super) type ResolveRegion = fn(&Lua, &Table) -> mlua::Result<RegionHandle>;

/// A widget's listener on its font instance, run after `SetFont` loads a new face or size and
/// after `SetFontObject` links another object: the notify `[vt+0x14]` that `0x79f342` and
/// `0x77e4d7` call, which the EditBox answers (`0x77e2a0`).
pub(super) type FontChanged = fn(&Lua, &Table) -> mlua::Result<()>;

/// The `%s` of a font verb's `Usage:` line: every shim passes the receiver's `GetName()`, or
/// `<unnamed>` (`0x84c7f0`) when it has none (`0x79d652`, `0x7a0315`, `0x780919`). Runs Lua, so a
/// caller asks before it borrows the model.
pub(super) fn usage_name(this: &Table) -> String {
    this.get::<mlua::Function>("GetName")
        .and_then(|f| f.call::<Option<String>>(this.clone()))
        .ok()
        .flatten()
        .unwrap_or_else(|| "<unnamed>".to_string())
}

/// `SetFont`'s three arguments as the shared implementation reads them.
pub(super) struct FontArgs {
    pub(super) path: String,
    pub(super) height: f32,
    /// The flags word written on every load (`0x79f334`): zero, no outline, when the gate fails.
    pub(super) flags: Outline,
}

/// `SetFont`'s argument gate: `0x79f210`, behind the Font (`0x7a0270`), EditBox (`0x797210`),
/// message-frame (`0x794450`, `0x791e80`), SimpleHTML (`0x795f30`) and Button (`0x780880`) shims,
/// and inlined in the FontString's (`0x79d582`-`0x79d5fb`). The path must pass `lua_isstring`
/// (`0x79f225`) and the height `lua_isnumber` (`0x79f239`), both coercing, else `Usage:
/// %s:SetFont("font", fontHeight [, flags])` (`0x87c69c`). The flags are read only when
/// `lua_isstring` passes (`0x79f287`), so a string or a number; `false`, nil or anything else is no
/// flags. An empty path is not an argument error but a failed load, which the caller answers with
/// nil.
pub(super) fn set_font_args(
    lua: &Lua,
    this: &Table,
    file: &Value,
    height: &Value,
    flags: &Value,
) -> mlua::Result<FontArgs> {
    let usage = || {
        mlua::Error::runtime(format!(
            "Usage: {}:SetFont(\"font\", fontHeight [, flags])",
            usage_name(this)
        ))
    };
    let Some(path) = binding_abi::optional_string(lua, file) else {
        return Err(usage());
    };
    let Some(height) = lua.coerce_number(height.clone())? else {
        return Err(usage());
    };
    // The Lua spelling (`OUTLINE`, `THICKOUTLINE`), not the XML one, parsed like `0x6f1a90`: a
    // case-insensitive substring scan, so `THICKOUTLINE` sets both.
    let flags =
        binding_abi::optional_string(lua, flags).map_or(Outline::None, |f| Outline::flags(&f));
    Ok(FontArgs {
        path,
        height: height as f32,
        flags,
    })
}

/// `SetSpacing`'s argument gate, the same on all six tables (`0x79fb54`, FontString `0x79e340`):
/// `lua_isnumber`, coercing, else `Usage: %s:SetSpacing(spacing)` (`0x87c708`).
pub(super) fn spacing_arg(lua: &Lua, this: &Table, v: &Value) -> mlua::Result<f32> {
    match lua.coerce_number(v.clone())? {
        Some(n) => Ok(n as f32),
        None => Err(mlua::Error::runtime(format!(
            "Usage: {}:SetSpacing(spacing)",
            usage_name(this)
        ))),
    }
}

/// Which implementations a table's font verbs reach: the FontString has its own `SetFont` load
/// (`0x79d4f0` → `0x7714f0`) and spacing pair (`0x79e2b0`, `0x79e1d0`, through the clamping
/// `0x772240`); the message frames' and the EditBox's font reach the shared ones (`0x79f210`,
/// `0x79fb40`, `0x79fbe0`), which keep a spacing as given.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FontKind {
    /// A FontString: its `GetSpacing` reads [`RegionData::line_gap`](super::RegionData::line_gap).
    FontString,
    /// A font instance: its `GetSpacing` reads the number as set.
    Instance,
}

/// A `SetFont` load's outcome.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Load {
    Loaded,
    /// A FontString's height of 0: no load, its font freed, still a success.
    Freed,
    Failed,
}

impl Load {
    fn verdict(ok: bool) -> Load {
        if ok {
            Load::Loaded
        } else {
            Load::Failed
        }
    }
}

/// Whether the font load (`0x44d040`, the factory `0x5c1ae0`) finds `path`: never for an empty one
/// (`0x44d059`), else the host's probe, as only the store knows the files; with no probe, any
/// non-empty path loads.
pub(super) fn face_loads(model: &Model, path: &str) -> bool {
    !path.is_empty() && model.font_probe.as_ref().is_none_or(|probe| probe(path))
}

/// The shared path's verdict (`0x79f210`): no load for a height of 0 or less
/// (`0x79f2f1`-`0x79f2ff`), else [`face_loads`]. A failure writes nothing (`0x79f30f` →
/// `0x79f361`). Borrows the model, so a caller asks before it holds it.
pub(super) fn instance_loads(lua: &Lua, path: &str, height: f32) -> bool {
    height > 0.0 && face_loads(&lua.app_data_ref::<Model>().expect("model"), path)
}

/// Install the twelve font-block methods onto `m`, reading `this` through `resolve`; only for a
/// widget whose reference method table carries all twelve.
pub(super) fn install(
    lua: &Lua,
    m: &Table,
    resolve: ResolveRegion,
    kind: FontKind,
    on_font_change: Option<FontChanged>,
) -> mlua::Result<()> {
    // ── the font object ──
    // SetFontObject(font | "font" | nil) → nothing (`0x79ef10`, usage string `0x87c5cc`). Any
    // other argument or an unknown name raises: every rejection is `luaL_error` (`0x6f4940`).
    m.set(
        "SetFontObject",
        lua.create_function(move |lua, (this, font): (Table, Value)| {
            let name = super::font::resolve("SetFontObject", &font)?;
            let rh = resolve(lua, &this)?;
            let relinked = {
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
                // The same object again changes nothing (`0x770c6a`).
                let relinked = d
                    .font_object
                    .as_deref()
                    .is_none_or(|old| !old.eq_ignore_ascii_case(&name));
                d.font_object = Some(name);
                // The inherit mask stays: each local setter clears its bit (`FONTINSTANCE+0x2c`)
                // and nothing restores it, so a property set locally survives a later
                // `SetFontObject`.
                super::font::repaint(d, &fo);
                model.touch_measure(rh);
                relinked
            };
            if relinked {
                if let Some(changed) = on_font_change {
                    changed(lua, &this)?;
                }
            }
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
    // alone), unlike `Button:SetFont` (`0x780880`), which returns nothing. A failed load writes
    // nothing: the shared path jumps past its stores (`0x79f30f` → `0x79f361`), the FontString's
    // returns before its own (`0x771571`-`0x77157b`).
    m.set(
        "SetFont",
        lua.create_function(
            move |lua, (this, file, height, flags): (Table, Value, Value, Value)| {
                let FontArgs {
                    path,
                    height,
                    flags,
                } = set_font_args(lua, &this, &file, &height, &flags)?;
                let rh = resolve(lua, &this)?;
                let mut model = lua.app_data_mut::<Model>().expect("model");
                let before = model
                    .region_data
                    .get(&rh)
                    .map(|d| (d.font_path.clone(), d.font_height, d.outline));
                let load = match kind {
                    // A FontString loads for any height but 0 (`0x77154d`-`0x77155b`); 0 skips the
                    // load and frees its font (`0x7715b3`), which still answers 1. What a negative
                    // height does inside the load (`0x44d040`) is untraced: the probe judges it.
                    FontKind::FontString if height == 0.0 => Load::Freed,
                    FontKind::FontString => Load::verdict(face_loads(&model, &path)),
                    // The shared path answers nil for a height of 0 or less without loading
                    // (`0x79f2f1`-`0x79f2ff`).
                    FontKind::Instance => Load::verdict(height > 0.0 && face_loads(&model, &path)),
                };
                if load == Load::Failed {
                    return Ok(Value::Nil);
                }
                let d = model.region_data.entry(rh).or_default();
                // Each argument is an explicit set, kept through a later font-object change
                // (`0x79f33a`, `0x79d625`). The flags are part of the font the load picks
                // (`0x44d040`), none included, and a freed font has none.
                if load == Load::Loaded {
                    d.font_path = Some(path);
                    d.outline = flags;
                } else {
                    d.font_path = None;
                    d.outline = Outline::None;
                }
                d.font_height = Some(height);
                d.font_explicit.face = true;
                d.font_explicit.outline = true;
                d.font_explicit.height = true;
                let after = (d.font_path.clone(), d.font_height, d.outline);
                model.touch_measure(rh);
                drop(model);
                // A load that changed the path, height or flags notifies (`0x79f32f` sets the
                // change bits, `0x79f342` calls the notify).
                if before.as_ref() != Some(&after) {
                    if let Some(changed) = on_font_change {
                        changed(lua, &this)?;
                    }
                }
                Ok(Value::Number(1.0))
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

    // ── the line spacing ──
    // SetSpacing(spacing) → nothing (`0x79fbbe`). The reference notifies on every call
    // (`0x79fbb1`-`0x79fbb9`) and the EditBox acts only on a set change bit (mask `0x11`,
    // `0x77e2b5`); here the notify runs only when the value changed.
    m.set(
        "SetSpacing",
        lua.create_function(move |lua, (this, v): (Table, Value)| {
            let spacing = spacing_arg(lua, &this, &v)?;
            let rh = resolve(lua, &this)?;
            let changed = {
                let mut model = lua.app_data_mut::<Model>().expect("model");
                let d = model.region_data.entry(rh).or_default();
                let changed = d.spacing.to_bits() != spacing.to_bits();
                d.spacing = spacing;
                // Clears inherit bit `0x10` (`0x79e37b`, `0x79fbb1`).
                d.font_explicit.spacing = true;
                model.touch_measure(rh);
                changed
            };
            if changed {
                if let Some(notify) = on_font_change {
                    notify(lua, &this)?;
                }
            }
            Ok(())
        })?,
    )?;
    // GetSpacing() → one number (`0x79fbe0`, FontString `0x79e1d0`).
    m.set(
        "GetSpacing",
        lua.create_function(move |lua, this: Table| {
            let rh = resolve(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            let d = model.region_data.get(&rh);
            Ok(match kind {
                FontKind::FontString => d.map_or(0.0, super::RegionData::line_gap),
                FontKind::Instance => d.map_or(0.0, |d| d.spacing),
            })
        })?,
    )?;

    Ok(())
}
