//! The `Minimap` methods: zoom, the mask, the ping and the model attributes. The reference has six
//! levels (`0x6da9a0`), a live index per mode (`0x86f698` outdoor, `0x86f69c` indoor) and the
//! `minimapZoom`/`minimapInsideZoom` CVars (default `"3"`) that re-seed it (`0x6d9008`); the app
//! maps the index to a radius (`0x6da9b0`) and holds the ping as a world point.

use mlua::{Lua, Table, Value};

use super::binding_abi::{lua_number_text, number_arg};
use super::object::{decode_id, frame_handle_of};
use super::Model;
use crate::widget::{
    FrameHandle, FrameKind, KindState, MinimapArt, MinimapArtSlot, MINIMAP_ZOOM_LEVELS,
};

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

/// The `CMinimap` ctor's nine `Model` children (`0x4edbc0`), made with the minimap as parent, so
/// at its stratum and its level plus one: five party and pet arrows (`0x4ede72`), three rim
/// arrows (`0x4edf1f`) and the player arrow (`0x4edfae`). Each is anchored CENTER on the
/// minimap's CENTER (`0x767c70(4, minimap, 4, x, y)`): the eight at offset 0 (`0x4ede8b`,
/// `0x4edf38`), each the disc's size (`[0xbc7970]` through slots 5 and 6, `0x4ede9e`, `0x4edeaf`),
/// model scale 0.6 (`0x4edeba`) and hidden (`0x4edece`, `0x4edf7b`); the five relevelled to the
/// minimap's level plus 2 (`0x4edee0`); the player arrow at offset 0.001 each way (`0x4edfd1`),
/// sized by its file, at model scale `G48 · 5/3` (`0x4edfec`) and left shown.
pub(crate) fn build_engine_children(lua: &Lua, minimap: FrameHandle) {
    use crate::layout::{Anchor, Point};
    let hide = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let Some((strata, level)) = model.arena.frame(minimap).map(|f| (f.strata, f.level)) else {
            return;
        };
        let base = super::worldmap_arrow::g48(&model) * (5.0 / 3.0);
        let unit = model.layout_unit();
        let units = model.layout_units();
        let minimap_id = model.frame_id(minimap);
        let disc = crate::widget::MINIMAP_DISC * base * unit;
        let mut children = Vec::with_capacity(crate::widget::MINIMAP_ENGINE_CHILDREN);
        for i in 0..crate::widget::MINIMAP_ENGINE_CHILDREN {
            let child = model.arena.create(FrameKind::Model, None, Some(minimap));
            model.arena.set_frame_strata(child, strata);
            let lift = if i < crate::widget::MINIMAP_GROUP_ARROWS {
                2
            } else {
                1
            };
            model
                .arena
                .set_frame_level(child, level.saturating_add(lift), false);
            let player = i + 1 == crate::widget::MINIMAP_ENGINE_CHILDREN;
            let offset = if player { 0.001 * unit } else { 0.0 };
            let input = model.layout_inputs.entry(child).or_default();
            input.anchors.set(
                Anchor::new(Point::Center, minimap_id, Point::Center, offset, offset),
                units,
            );
            if !player {
                input.width = disc;
                input.height = disc;
            }
            model.invalidate_frame(child, true);
            if let Some(KindState::Model(m)) =
                model.arena.frame_mut(child).map(|f| &mut f.kind_state)
            {
                m.scale = if player {
                    base
                } else {
                    crate::widget::MINIMAP_ARROW_SCALE
                };
            }
            children.push(child);
        }
        let (arrows, player) = children.split_at(crate::widget::MINIMAP_ENGINE_CHILDREN - 1);
        if let Some(KindState::Minimap(m)) =
            model.arena.frame_mut(minimap).map(|f| &mut f.kind_state)
        {
            m.group_arrows = arrows[..crate::widget::MINIMAP_GROUP_ARROWS].to_vec();
            m.rim_arrows = arrows[crate::widget::MINIMAP_GROUP_ARROWS..].to_vec();
            m.player_arrow = player.first().copied();
            m.level_base = i32::from(level) + 3;
            m.base = base;
        }
        arrows.to_vec()
    };
    for child in hide {
        super::visibility::set_shown(lua, child, false);
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

/// The texture setters' one body (`SetMaskTexture` `0x4ee4a0`): the member prologue, then
/// `lua_isstring(2)` (`0x4ee532`), whose failure raises `Usage: %s:<name>("file")` (`0x84c7fc`)
/// with the frame's name or `<unnamed>`. Then `TextureCreate` on `lua_tostring(2)` (`0x4ee5e4`),
/// its result stored over the old handle (`0x4ee5e9`) and, when the load logged at severity 2 or
/// more (`0x4ee5ee`), `%s:<name>(): Couldn't load the file %s` (`0x84c7c0`, `0x4ee623`) after the
/// store. An empty name loads nothing and logs nothing. With no host probe the file is taken.
fn set_art(
    lua: &Lua,
    this: &Value,
    path: Value,
    slot: MinimapArtSlot,
    name: &str,
) -> mlua::Result<()> {
    let h = this_minimap(lua, this)?;
    let path = match path {
        Value::String(s) => s.to_string_lossy(),
        Value::Integer(i) => lua_number_text(i as f64),
        Value::Number(n) => lua_number_text(n),
        _ => {
            return Err(mlua::Error::runtime(format!(
                "Usage: {}:{name}(\"file\")",
                frame_name(lua, h)
            )));
        }
    };
    let art = if path.is_empty() {
        MinimapArt::Null
    } else {
        match super::region::texture_name_verdict(
            &lua.app_data_ref::<Model>().expect("model app_data"),
            &path,
        ) {
            super::region::FileLoad::Failed => MinimapArt::Missing,
            _ => MinimapArt::File(path.clone()),
        }
    };
    let failed = art == MinimapArt::Missing;
    lua.app_data_mut::<Model>()
        .expect("model app_data")
        .minimap
        .set_art(slot, art);
    if failed {
        return Err(mlua::Error::runtime(format!(
            "{}:{name}(): Couldn't load the file {path}",
            frame_name(lua, h)
        )));
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
        "SetMaskTexture",
        lua.create_function(|lua, (this, path): (Value, Value)| {
            set_art(lua, &this, path, MinimapArtSlot::Mask, "SetMaskTexture")
        })?,
    )?;
    m.set(
        // The landmark atlas `0xbc827c` (`0x4ee640`: store `0x4ee789`, texts `0x84c850`, `0x84c820`),
        // drawn as a fixed 8×8 grid whatever its size.
        "SetIconTexture",
        lua.create_function(|lua, (this, path): (Value, Value)| {
            set_art(lua, &this, path, MinimapArtSlot::Icons, "SetIconTexture")
        })?,
    )?;
    m.set(
        // The dot atlas `0xbc822c` (`0x4ee7e0`: store `0x4ee929`, texts `0x84c8a4`, `0x84c874`),
        // drawn as a fixed 4×4 grid of quarters.
        "SetBlipTexture",
        lua.create_function(|lua, (this, path): (Value, Value)| {
            set_art(lua, &this, path, MinimapArtSlot::Blips, "SetBlipTexture")
        })?,
    )?;
    m.set(
        "GetZoomLevels",
        // The reference's constant (`0x6da9a0`), with no `this` lookup (`0x4eebe0`).
        lua.create_function(|_, _: mlua::MultiValue| Ok(MINIMAP_ZOOM_LEVELS))?,
    )?;
    m.set(
        "PingLocation",
        // `0x4eeca0`: nothing without the player object (`0x4eecdf`). Arguments 2 and 3 count only
        // when both pass `lua_isnumber` (`0x4eed0b`, `0x4eed1f`), and only then is `this` looked
        // up (`0x4eed25`); otherwise both offsets stay 0 and the ping lands on the player. The
        // first is negated (`0x4eedeb`) and divided by the frame's given height (slot 0x20,
        // `0x4eee31`), the second by its width (slot 0x1c, `0x4eee21`): fractions of its own size,
        // whatever its scale. Each times the zoom diameter (`0x4eecf8`) offsets the player's point,
        // world X by the second and world Y by the first (`0x4eee48`), with no disc test. The
        // party send (`0x4eef72`) goes to the host; the ping then applies at once (`0x4eefb5`).
        lua.create_function(|lua, args: mlua::MultiValue| {
            let player = lua
                .app_data_ref::<Model>()
                .expect("model app_data")
                .minimap
                .player;
            let Some(player) = player else {
                return Ok(());
            };
            let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Nil);
            let (x, y) = (lua.coerce_number(arg(1))?, lua.coerce_number(arg(2))?);
            let (mut first, mut second) = (0.0_f64, 0.0_f64);
            if let (Some(x), Some(y)) = (x, y) {
                let h = this_minimap(lua, &arg(0))?;
                let (w, ht) = {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    model
                        .layout_inputs
                        .get(&h)
                        .map_or((0.0, 0.0), |i| (i.width, i.height))
                };
                first = -x / f64::from(ht);
                second = y / f64::from(w);
            }
            super::tick::verb(lua, |model| {
                let d = 2.0 * f64::from(model.minimap.radius_yd());
                let world = (
                    (f64::from(player.0) + second * d) as f32,
                    (f64::from(player.1) + first * d) as f32,
                );
                model.minimap_pings_sent.push(world);
                if let Some((a, b)) = model.minimap.apply_ping(world) {
                    model.raise("MINIMAP_PING", super::ping_args("player", a, b));
                }
            });
            Ok(())
        })?,
    )?;
    m.set(
        "GetPingPosition",
        // Two numbers recomputed from the stored point at every call, `(0, 0)` with no player
        // (`0x4eefd0`); the stock `Minimap_OnUpdate` multiplies them for 5 s, so a nil would error
        // every frame. No `this` lookup.
        lua.create_function(|lua, _: mlua::MultiValue| {
            Ok(lua
                .app_data_ref::<Model>()
                .expect("model app_data")
                .minimap
                .ping_position())
        })?,
    )?;
    lua.set_named_registry_value(REG_MINIMAP_METHODS, m)?;
    Ok(())
}
