//! Frame methods: visibility, hierarchy, strata, level, scale, alpha, backdrop and input flags.

use mlua::{Lua, Table, Value};

use crate::script::region_map::{set_shared, Side};

use crate::order::Strata;
use crate::script::{event, Backdrop, Insets, Model};
use crate::widget::FrameKind;

use super::{decode_id, draw_layer_from_str, frame_handle_of, frame_wrapper, strata_from_str};

/// Populate `m`'s visibility, hierarchy, strata, backdrop and input methods.
pub(super) fn install(lua: &Lua, m: &Table) -> mlua::Result<()> {
    // Show / Hide / visibility. No `SetShown`: no 1.12 method table registers it.
    m.set(
        "Show",
        lua.create_function(|lua, this: Table| set_shown(lua, &this, true))?,
    )?;
    m.set(
        "Hide",
        lua.create_function(|lua, this: Table| set_shown(lua, &this, false))?,
    )?;
    m.set(
        "IsShown",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(crate::script::binding_abi::flag(
                model.arena.frame(h).map(|f| f.shown).unwrap_or(false),
            ))
        })?,
    )?;
    m.set(
        "IsVisible",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(crate::script::binding_abi::flag(
                model
                    .arena
                    .frame(h)
                    .map(|f| f.effective_visible)
                    .unwrap_or(false),
            ))
        })?,
    )?;
    // Identity / hierarchy
    set_shared(lua, m, Side::Frame, "GetName", |lua, this: Table| {
        let h = frame_handle_of(lua, &this)?;
        let name = {
            let model = lua.app_data_ref::<Model>().expect("model");
            model.arena.frame(h).and_then(|f| f.name.clone())
        };
        match name {
            Some(n) => Ok(Value::String(lua.create_string(&n)?)),
            None => Ok(Value::Nil),
        }
    })?;
    // GetID/SetID (`0x775280`/`0x775340`): a plain numeric label at `+0xb0` (XML `id=`), default 0.
    m.set(
        "GetID",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(model.arena.frame(h).map(|f| f.wow_id).unwrap_or(0))
        })?,
    )?;
    m.set(
        "SetID",
        lua.create_function(|lua, (this, id): (Table, i64)| {
            let h = frame_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            if let Some(f) = model.arena.frame_mut(h) {
                f.wow_id = id;
            }
            Ok(())
        })?,
    )?;
    // ── GetObjectType / IsObjectType, frame side (`0x7a11d0`/`0x7a1290`) ──
    //
    // Each class's chain is a fixed list in the reference, so a table here. `ScrollingMessageFrame`
    // derives from `Frame`, not `MessageFrame` (`0x787940`); `SimpleHTML` keeps its capitals,
    // unlike the `SimpleHtml` variant; there is no `Cooldown` type (the reference's is a `Model`).
    fn type_chain(kind: FrameKind) -> &'static [&'static str] {
        match kind {
            FrameKind::Frame => &["Frame", "Region"],
            FrameKind::WorldFrame => &["Frame", "Region"],
            FrameKind::Button => &["Button", "Frame", "Region"],
            FrameKind::CheckButton => &["CheckButton", "Button", "Frame", "Region"],
            // `CLootButton::IsObjectType` (`0x495af0`) prepends its name to `Button`'s chain.
            FrameKind::LootButton => &["LootButton", "Button", "Frame", "Region"],
            FrameKind::EditBox => &["EditBox", "Frame", "Region"],
            FrameKind::StatusBar => &["StatusBar", "Frame", "Region"],
            FrameKind::Slider => &["Slider", "Frame", "Region"],
            FrameKind::ScrollFrame => &["ScrollFrame", "Frame", "Region"],
            FrameKind::Model => &["Model", "Frame", "Region"],
            FrameKind::PlayerModel => &["PlayerModel", "Model", "Frame", "Region"],
            FrameKind::DressUpModel => &["DressUpModel", "PlayerModel", "Model", "Frame", "Region"],
            FrameKind::TabardModel => &["TabardModel", "PlayerModel", "Model", "Frame", "Region"],
            FrameKind::MessageFrame => &["MessageFrame", "Frame", "Region"],
            FrameKind::ScrollingMessageFrame => &["ScrollingMessageFrame", "Frame", "Region"],
            FrameKind::ColorSelect => &["ColorSelect", "Frame", "Region"],
            FrameKind::SimpleHtml => &["SimpleHTML", "Frame", "Region"],
            FrameKind::MovieFrame => &["MovieFrame", "Frame", "Region"],
            FrameKind::GameTooltip => &["GameTooltip", "Frame", "Region"],
            FrameKind::Minimap => &["Minimap", "Frame", "Region"],
        }
    }
    fn chain_of(lua: &Lua, this: &Table) -> mlua::Result<&'static [&'static str]> {
        let h = frame_handle_of(lua, this)?;
        let model = lua.app_data_ref::<Model>().expect("model");
        Ok(model
            .arena
            .frame(h)
            .map_or(&["Frame", "Region"][..], |f| type_chain(f.kind)))
    }

    set_shared(lua, m, Side::Frame, "GetObjectType", |lua, this: Table| {
        Ok(Value::String(lua.create_string(chain_of(lua, &this)?[0])?))
    })?;

    // A case-insensitive whole-string match answering 1 or nil; a non-string, non-number argument
    // raises the reference's `Usage:`.
    set_shared(
        lua,
        m,
        Side::Frame,
        "IsObjectType",
        |lua, (this, want): (Table, Value)| {
            let chain = chain_of(lua, &this)?;
            let want = match &want {
                Value::String(s) => s.to_str()?.to_string(),
                Value::Number(n) => n.to_string(),
                Value::Integer(i) => i.to_string(),
                _ => {
                    let h = frame_handle_of(lua, &this)?;
                    let model = lua.app_data_ref::<Model>().expect("model");
                    let who = model
                        .arena
                        .frame(h)
                        .and_then(|f| f.name.clone())
                        .unwrap_or_else(|| "<unnamed>".to_string());
                    return Err(mlua::Error::runtime(format!(
                        "Usage: {who}:IsObjectType(\"TYPE\")"
                    )));
                }
            };
            Ok(if chain.iter().any(|t| want.eq_ignore_ascii_case(t)) {
                Value::Number(1.0)
            } else {
                Value::Nil
            })
        },
    )?;

    // ── GetFrameType / IsFrameType ──
    //
    // The frame script's own spellings of the pair above (`GetFrameType 0x773640`, `IsFrameType
    // 0x773700`), reading the same per-class type slot: one name, or 1 or nil. An absent argument
    // raises `Usage:` like `IsObjectType`; the reference's absent-argument branch is untraced.
    m.set(
        "GetFrameType",
        lua.create_function(|lua, this: Table| {
            Ok(Value::String(lua.create_string(chain_of(lua, &this)?[0])?))
        })?,
    )?;
    m.set(
        "IsFrameType",
        lua.create_function(|lua, (this, want): (Table, Value)| {
            let chain = chain_of(lua, &this)?;
            let want = match &want {
                Value::String(s) => s.to_str()?.to_string(),
                Value::Number(n) => n.to_string(),
                Value::Integer(i) => i.to_string(),
                _ => {
                    let h = frame_handle_of(lua, &this)?;
                    let model = lua.app_data_ref::<Model>().expect("model");
                    let who = model
                        .arena
                        .frame(h)
                        .and_then(|f| f.name.clone())
                        .unwrap_or_else(|| "<unnamed>".to_string());
                    return Err(mlua::Error::runtime(format!(
                        "Usage: {who}:IsFrameType(\"TYPE\")"
                    )));
                }
            };
            Ok(if chain.iter().any(|t| want.eq_ignore_ascii_case(t)) {
                Value::Number(1.0)
            } else {
                Value::Nil
            })
        })?,
    )?;

    set_shared(lua, m, Side::Frame, "GetParent", |lua, this: Table| {
        let h = frame_handle_of(lua, &this)?;
        let parent_id = {
            let mut model = lua.app_data_mut::<Model>().expect("model");
            model
                .arena
                .frame(h)
                .and_then(|f| f.parent)
                .map(|p| model.frame_id(p))
        };
        match parent_id {
            Some(pid) => Ok(Value::Table(frame_wrapper(lua, pid)?)),
            None => Ok(Value::Nil),
        }
    })?;
    // ── GetChildren / GetNumChildren / GetRegions / GetNumRegions ──
    //
    // `0x774180` / `0x774080` / `0x773f60` / `0x773e60`: link order, oldest first (both linkers,
    // `0x76a750` and `0x76aa20`, append at the tail), hidden ones included; the list as separate
    // values, none when empty, the count as one number. Each count shares its list's walk.
    m.set(
        "GetChildren",
        lua.create_function(|lua, this: Table| {
            let ids = children_of(lua, &this)?;
            let mut out = Vec::with_capacity(ids.len());
            for id in ids {
                out.push(Value::Table(frame_wrapper(lua, id)?));
            }
            Ok(mlua::MultiValue::from_vec(out))
        })?,
    )?;
    m.set(
        "GetNumChildren",
        lua.create_function(|lua, this: Table| Ok(children_of(lua, &this)?.len()))?,
    )?;
    m.set(
        "GetRegions",
        lua.create_function(|lua, this: Table| {
            let ids = regions_of(lua, &this)?;
            let mut out = Vec::with_capacity(ids.len());
            for id in ids {
                out.push(Value::Table(crate::script::region::region_wrapper(
                    lua, id,
                )?));
            }
            Ok(mlua::MultiValue::from_vec(out))
        })?,
    )?;
    m.set(
        "GetNumRegions",
        lua.create_function(|lua, this: Table| Ok(regions_of(lua, &this)?.len()))?,
    )?;
    // `SetParent` (`0x7a1550`): a frame, a name (`0x76c760`) or an explicit nil; an absent argument
    // raises like an unknown name (`0x6f3400` gives -1), and so does a cycle (`0x87cb14`). As in
    // the reference, hide fires before show, between `reparent_begin` and `reparent_finish`,
    // since `fire_visibility_changes` reads each frame's live state for the direction.
    set_shared(
        lua,
        m,
        Side::Frame,
        "SetParent",
        |lua, args: mlua::MultiValue| {
            let mut it = args.into_iter();
            let this = match it.next() {
                Some(Value::Table(t)) => t,
                _ => return Err(mlua::Error::runtime("expected a frame")),
            };
            let h = frame_handle_of(lua, &this)?;
            let who = {
                let model = lua.app_data_ref::<Model>().expect("model");
                model
                    .arena
                    .frame(h)
                    .and_then(|f| f.name.clone())
                    .unwrap_or_else(|| "<unnamed>".to_string())
            };
            let parent_arg = it.next();
            // `_G[name]` is read with no guard alive and no `$parent` expansion: `0x7a1550` calls
            // `0x76c760` directly, and only the layout path's `0x76c700` expands the token.
            let named = match &parent_arg {
                Some(Value::String(s)) => Some(match s.to_str() {
                    Ok(n) => super::prefetch_named_target(lua, n.as_ref(), None),
                    Err(_) => super::NamedTarget::unreadable(),
                }),
                _ => None,
            };
            let new_parent = {
                let model = lua.app_data_ref::<Model>().expect("model");
                match &parent_arg {
                    // An explicit nil reparents to the screen root, resetting strata and level.
                    Some(Value::Nil) => None,
                    Some(Value::Table(t)) => Some(
                        decode_id(t)
                            .ok()
                            .and_then(|id| model.id_to_frame.get(&id).copied())
                            .ok_or_else(|| {
                                mlua::Error::runtime(format!(
                                    "{who}:SetParent(): Couldn't find region named ''"
                                ))
                            })?,
                    ),
                    Some(Value::String(_)) => {
                        // `0x76c760` accepts only a frame (`[0xcf0c10]`), not any region
                        // (`[0xcf0c3c]`), so a region's name fails like an absent one.
                        let nt = named.as_ref().expect("a String argument is prefetched");
                        let hit = super::resolve_named_target(&model, nt)
                            .and_then(|id| model.id_to_frame.get(&id).copied());
                        Some(hit.ok_or_else(|| {
                            mlua::Error::runtime(format!(
                                "{who}:SetParent(): Couldn't find region named '{}'",
                                nt.name
                            ))
                        })?)
                    }
                    // Absent, or any other type: the reference raises first (`0x87cb48`).
                    _ => {
                        return Err(mlua::Error::runtime(format!(
                            "{who}:SetParent(): Couldn't find region named ''"
                        )))
                    }
                }
            };
            // The cycle guard raises (`0x7a177f`'s ancestor walk, the frame itself included).
            if let Some(np) = new_parent {
                let model = lua.app_data_ref::<Model>().expect("model");
                if np == h || model.arena.is_ancestor(h, np) {
                    let pname = model
                        .arena
                        .frame(np)
                        .and_then(|f| f.name.clone())
                        .unwrap_or_else(|| "<unnamed>".to_string());
                    return Err(mlua::Error::runtime(format!(
                        "{who}:SetParent(): Would create a loop parenting to {pname}"
                    )));
                }
            }
            let hidden = {
                let mut model = lua.app_data_mut::<Model>().expect("model");
                model.arena.reparent_begin(h, new_parent)
            };
            // None: the same parent, a total no-op (`0x76ab20`), not even a layout touch.
            let Some(hidden) = hidden else { return Ok(()) };
            let was_visible = !hidden.is_empty();
            event::fire_visibility_changes(lua, hidden);
            let shown = {
                let mut model = lua.app_data_mut::<Model>().expect("model");
                let shown = model.arena.reparent_finish(h, new_parent, was_visible);
                // A reparent moves the subtree's effective scale, a layout input and part of every
                // descendant FontString's measure key.
                model.touch_layout_reparent(h);
                model.touch_measure_all();
                shown
            };
            event::fire_visibility_changes(lua, shown);
            Ok(())
        },
    )?;
    // Strata / level / scale / alpha
    m.set(
        "SetFrameStrata",
        lua.create_function(|lua, (this, strata): (Table, String)| {
            let h = frame_handle_of(lua, &this)?;
            let s = strata_from_str(&strata)
                .ok_or_else(|| mlua::Error::runtime(format!("unknown frameStrata '{strata}'")))?;
            lua.app_data_mut::<Model>()
                .expect("model")
                .arena
                .set_frame_strata(h, s);
            Ok(())
        })?,
    )?;
    m.set(
        "GetFrameStrata",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            let s = model.arena.frame(h).map(|f| f.strata).unwrap_or_default();
            Ok(strata_name(s).to_string())
        })?,
    )?;
    m.set(
        "SetFrameLevel",
        lua.create_function(|lua, (this, level): (Table, i64)| {
            let h = frame_handle_of(lua, &this)?;
            let lvl = level.clamp(0, i64::from(u16::MAX)) as u16;
            // A script level change moves no children: `0x774560` calls `set_frame_level 0x76a4f0`
            // with `propagate = 0`, which stock `BonusActionButtonTemplate` relies on.
            lua.app_data_mut::<Model>()
                .expect("model")
                .arena
                .set_frame_level(h, lvl, false);
            Ok(())
        })?,
    )?;
    m.set(
        "GetFrameLevel",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(i64::from(
                model.arena.frame(h).map(|f| f.level).unwrap_or(0),
            ))
        })?,
    )?;
    m.set(
        "SetScale",
        lua.create_function(|lua, (this, scale): (Table, f32)| {
            let h = frame_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let changed = model.arena.frame(h).is_some_and(|f| f.scale != scale);
            model.arena.set_scale(h, scale);
            if changed {
                model.touch_layout();
                // The owner's effective scale is in every descendant FontString's measure key.
                model.touch_measure_all();
            }
            Ok(())
        })?,
    )?;
    m.set(
        "GetScale",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(model.arena.frame(h).map(|f| f.scale).unwrap_or(1.0))
        })?,
    )?;
    m.set(
        "SetAlpha",
        lua.create_function(|lua, (this, alpha): (Table, f32)| {
            let h = frame_handle_of(lua, &this)?;
            // The reference clamps to 0..1; stock fade code passing a 0..255 alpha relies on it.
            lua.app_data_mut::<Model>()
                .expect("model")
                .arena
                .set_alpha(h, alpha.clamp(0.0, 1.0));
            Ok(())
        })?,
    )?;
    m.set(
        "GetAlpha",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(model.arena.frame(h).map(|f| f.alpha).unwrap_or(1.0))
        })?,
    )?;
    // `SetBackdrop(table|nil)` (`0x7776e0` → `0x76a5d0`) installs or removes the tiled background
    // and 8-piece border, both white until tinted; the two colour setters tint the background and
    // the border respectively (`0x77f410`/`0x77f440`).
    m.set(
        "SetBackdrop",
        lua.create_function(|lua, (this, arg): (Table, Value)| {
            let h = frame_handle_of(lua, &this)?;
            let bd = match arg {
                Value::Nil => None,
                Value::Table(t) => Some(backdrop_from_table(&t)?),
                other => {
                    return Err(mlua::Error::runtime(format!(
                        "SetBackdrop: expected a table or nil, got {}",
                        other.type_name()
                    )))
                }
            };
            let mut model = lua.app_data_mut::<Model>().expect("model");
            match bd {
                Some(b) => {
                    model.backdrops.insert(h, b);
                }
                None => {
                    model.backdrops.remove(&h);
                }
            }
            Ok(())
        })?,
    )?;
    // `GetBackdrop([table])` (`0x777370`) rebuilds the table from the stored struct: no backdrop is
    // zero values, not nil; an omitted key reads its ctor default (files `""`, `tileSize` 0,
    // `edgeSize` 32; `0x77e5f0`); `tile` is 1 or absent, never a boolean; a table argument is
    // filled in place, its `insets` reused (`0x77740e`). No colour keys.
    m.set(
        "GetBackdrop",
        lua.create_function(|lua, (this, target): (Table, Value)| {
            let h = frame_handle_of(lua, &this)?;
            // Copied out before any Lua write: filling a caller's table can run a `__newindex` that
            // re-enters and would panic on the app-data borrow.
            let bd = {
                let model = lua.app_data_ref::<Model>().expect("model");
                model.backdrops.get(&h).cloned()
            };
            let Some(bd) = bd else {
                return Ok(mlua::MultiValue::new());
            };
            let t = match target {
                Value::Table(t) => t,
                _ => lua.create_table()?,
            };
            t.set("bgFile", bd.bg_file.as_deref().unwrap_or(""))?;
            t.set("edgeFile", bd.edge_file.as_deref().unwrap_or(""))?;
            t.set(
                "tile",
                if bd.tile {
                    Value::Number(1.0)
                } else {
                    Value::Nil // erases the key from a reused table
                },
            )?;
            t.set("tileSize", f64::from(bd.tile_size))?;
            t.set("edgeSize", f64::from(bd.edge_size))?;
            let insets = match t.get::<Value>("insets") {
                Ok(Value::Table(existing)) => existing,
                _ => {
                    let fresh = lua.create_table()?;
                    t.set("insets", &fresh)?;
                    fresh
                }
            };
            insets.set("left", f64::from(bd.insets.left))?;
            insets.set("right", f64::from(bd.insets.right))?;
            insets.set("top", f64::from(bd.insets.top))?;
            insets.set("bottom", f64::from(bd.insets.bottom))?;
            Ok(mlua::MultiValue::from_vec(vec![Value::Table(t)]))
        })?,
    )?;
    m.set(
        "SetBackdropColor",
        lua.create_function(|lua, (this, args): (Table, mlua::MultiValue)| {
            let color = backdrop_color(lua, args);
            let h = frame_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            if let Some(bd) = model.backdrops.get_mut(&h) {
                bd.bg_color = color;
            }
            Ok(())
        })?,
    )?;
    m.set(
        "SetBackdropBorderColor",
        lua.create_function(|lua, (this, args): (Table, mlua::MultiValue)| {
            let color = backdrop_color(lua, args);
            let h = frame_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            if let Some(bd) = model.backdrops.get_mut(&h) {
                bd.border_color = color;
            }
            Ok(())
        })?,
    )?;
    m.set(
        "GetBackdropColor",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            match model.backdrops.get(&h) {
                Some(bd) => Ok((
                    bd.bg_color[0],
                    bd.bg_color[1],
                    bd.bg_color[2],
                    bd.bg_color[3],
                )),
                None => Ok((1.0, 1.0, 1.0, 1.0)),
            }
        })?,
    )?;
    m.set(
        "GetBackdropBorderColor",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            match model.backdrops.get(&h) {
                Some(bd) => Ok((
                    bd.border_color[0],
                    bd.border_color[1],
                    bd.border_color[2],
                    bd.border_color[3],
                )),
                None => Ok((1.0, 1.0, 1.0, 1.0)),
            }
        })?,
    )?;
    // Mouse interaction: `EnableMouse` gates hit-testing.
    m.set(
        "EnableMouse",
        lua.create_function(|lua, (this, enable): (Table, bool)| {
            let h = frame_handle_of(lua, &this)?;
            lua.app_data_mut::<Model>()
                .expect("model")
                .arena
                .set_mouse_enabled(h, enable);
            Ok(())
        })?,
    )?;
    m.set(
        "IsMouseEnabled",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(crate::script::binding_abi::flag(
                model.arena.is_mouse_enabled(h),
            ))
        })?,
    )?;
    // EnableKeyboard / IsKeyboardEnabled (`0x776ec0`/`0x776f90`): the flag key delivery gates on
    // (`script::keyboard`), separate from having a handler (`0x76af00`).
    m.set(
        "EnableKeyboard",
        lua.create_function(|lua, (this, enable): (Table, bool)| {
            let h = frame_handle_of(lua, &this)?;
            lua.app_data_mut::<Model>()
                .expect("model")
                .arena
                .set_keyboard_enabled(h, enable);
            Ok(())
        })?,
    )?;
    m.set(
        "IsKeyboardEnabled",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(crate::script::binding_abi::flag(
                model.arena.is_keyboard_enabled(h),
            ))
        })?,
    )?;
    // EnableDrawLayer / DisableDrawLayer (`0x7755b0`/`0x775680`): the frame's layer mask. An
    // unknown layer name is a no-op here; the reference's handling of one is untraced.
    for (name, disable) in [("EnableDrawLayer", false), ("DisableDrawLayer", true)] {
        m.set(
            name,
            lua.create_function(move |lua, (this, layer): (Table, Value)| {
                let Some(l) = layer
                    .as_string()
                    .and_then(|s| s.to_str().ok().and_then(|s| draw_layer_from_str(&s)))
                else {
                    return Ok(());
                };
                let h = frame_handle_of(lua, &this)?;
                let mut model = lua.app_data_mut::<Model>().expect("model");
                if let Some(frame) = model.arena.frame_mut(h) {
                    let bit = 1u8 << l.index();
                    if disable {
                        frame.disabled_layers |= bit;
                    } else {
                        frame.disabled_layers &= !bit;
                    }
                }
                Ok(())
            })?,
        )?;
    }
    // EnableMouseWheel / IsMouseWheelEnabled: the wheel's own flag, separate from `EnableMouse` as
    // in the reference, and the one the wheel hit test gates on.
    m.set(
        "EnableMouseWheel",
        lua.create_function(|lua, (this, enable): (Table, bool)| {
            let h = frame_handle_of(lua, &this)?;
            lua.app_data_mut::<Model>()
                .expect("model")
                .arena
                .set_mouse_wheel_enabled(h, enable);
            Ok(())
        })?,
    )?;
    m.set(
        "IsMouseWheelEnabled",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(crate::script::binding_abi::flag(
                model.arena.is_mouse_wheel_enabled(h),
            ))
        })?,
    )?;
    // Clamp-to-screen (`0x776c00`/`0x776cb0`, geometry flags bit 4): the layout resolve keeps the
    // frame's rect inside the window, size preserved.
    m.set(
        "SetClampedToScreen",
        lua.create_function(|lua, (this, clamp): (Table, bool)| {
            let h = frame_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let changed = model.arena.is_clamped_to_screen(h) != clamp;
            model.arena.set_clamped_to_screen(h, clamp);
            if changed {
                model.touch_layout();
            }
            Ok(())
        })?,
    )?;
    m.set(
        "IsClampedToScreen",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(crate::script::binding_abi::flag(
                model.arena.is_clamped_to_screen(h),
            ))
        })?,
    )?;
    // Hit-rect insets shrink only the mouse hit rect; geometry, drawing and anchors ignore them.
    m.set(
        "SetHitRectInsets",
        lua.create_function(
            |lua, (this, left, right, top, bottom): (Table, f32, f32, f32, f32)| {
                let h = frame_handle_of(lua, &this)?;
                lua.app_data_mut::<Model>()
                    .expect("model")
                    .arena
                    .set_hit_rect_insets(h, [left, right, top, bottom]);
                Ok(())
            },
        )?,
    )?;
    m.set(
        "GetHitRectInsets",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            let i = model.arena.hit_rect_insets(h);
            Ok((i[0], i[1], i[2], i[3]))
        })?,
    )?;
    Ok(())
}

/// Parse a `SetBackdrop` table (`0x7776e0`): exactly the reference's keys, a wrong-typed one read
/// as absent and a missing one left at the ctor default (`edgeSize` 32); no colour keys.
fn backdrop_from_table(t: &Table) -> mlua::Result<Backdrop> {
    let mut bd = Backdrop::default();
    if let Ok(Value::String(s)) = t.get::<Value>("bgFile") {
        bd.bg_file = Some(s.to_str()?.to_string());
    }
    if let Ok(Value::String(s)) = t.get::<Value>("edgeFile") {
        bd.edge_file = Some(s.to_str()?.to_string());
    }
    bd.tile = !matches!(
        t.get::<Value>("tile").unwrap_or(Value::Nil),
        Value::Nil | Value::Boolean(false)
    );
    if let Ok(Value::Number(n)) = t.get::<Value>("tileSize") {
        bd.tile_size = n as f32;
    } else if let Ok(Value::Integer(n)) = t.get::<Value>("tileSize") {
        bd.tile_size = n as f32;
    }
    if let Ok(Value::Number(n)) = t.get::<Value>("edgeSize") {
        bd.edge_size = n as f32;
    } else if let Ok(Value::Integer(n)) = t.get::<Value>("edgeSize") {
        bd.edge_size = n as f32;
    }
    if let Ok(Value::Table(ins)) = t.get::<Value>("insets") {
        let num = |k: &str| -> f32 {
            match ins.get::<Value>(k) {
                Ok(Value::Number(n)) => n as f32,
                Ok(Value::Integer(n)) => n as f32,
                _ => 0.0,
            }
        };
        bd.insets = Insets {
            left: num("left"),
            right: num("right"),
            top: num("top"),
            bottom: num("bottom"),
        };
    }
    Ok(bd)
}

fn strata_name(s: Strata) -> &'static str {
    match s {
        Strata::World => "WORLD",
        Strata::Background => "BACKGROUND",
        Strata::Low => "LOW",
        Strata::Medium => "MEDIUM",
        Strata::High => "HIGH",
        Strata::Dialog => "DIALOG",
        Strata::Fullscreen => "FULLSCREEN",
        Strata::FullscreenDialog => "FULLSCREEN_DIALOG",
        Strata::Tooltip => "TOOLTIP",
        Strata::Blizzard => "BLIZZARD",
    }
}

fn set_shown(lua: &Lua, this: &Table, shown: bool) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let changed = {
        let mut model = lua.app_data_mut::<Model>().expect("model");
        model.arena.set_shown(h, shown)
    };
    event::fire_visibility_changes(lua, changed);
    Ok(())
}

/// The frame's child ids in link order, the one walk `GetChildren` and `GetNumChildren` share.
fn children_of(lua: &Lua, this: &Table) -> mlua::Result<Vec<u32>> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model");
    let Some(children) = model.arena.frame(h).map(|f| f.children.clone()) else {
        return Ok(Vec::new());
    };
    Ok(children.into_iter().map(|c| model.frame_id(c)).collect())
}

/// The frame's region ids in link order, the one walk `GetRegions` and `GetNumRegions` share. A
/// detached region is left out (`SetParent(nil)` unlinks it, `0x76a7f0`), and so is the title
/// region, which never reaches the linker `0x77fd10` (`0x773910`, `0x769b79`); a Button's label
/// and state textures are in.
fn regions_of(lua: &Lua, this: &Table) -> mlua::Result<Vec<u32>> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model");
    let Some((regions, title)) = model
        .arena
        .frame(h)
        .map(|f| (f.regions.clone(), f.title_region))
    else {
        return Ok(Vec::new());
    };
    let live: Vec<_> = regions
        .into_iter()
        .filter(|r| Some(*r) != title)
        .filter(|r| model.arena.region(*r).is_some_and(|reg| !reg.detached))
        .collect();
    Ok(live.into_iter().map(|r| model.region_id(r)).collect())
}

/// One backdrop colour channel as `SetBackdropColor` (`0x777d30`) and `SetBackdropBorderColor`
/// (`0x7780d0`) convert it: r, g, b through a bare `lua_tonumber` (nil is 0), alpha gated by
/// `lua_isnumber` with 1.0 staged (`0x778220`, `0x778227`); clamped to 0..1 with NaN at 1, then
/// stored as a byte, rounded half up (`×255 + 0.5`, `__ftol`).
fn backdrop_channel(lua: &Lua, v: Option<Value>, gated: bool) -> f32 {
    let x = if gated {
        v.and_then(|v| lua.coerce_number(v).ok().flatten())
            .unwrap_or(1.0)
    } else {
        crate::script::binding_abi::coerced_number(lua, v)
    };
    let clamped = if x.is_nan() { 1.0 } else { x.clamp(0.0, 1.0) };
    let byte = (clamped * 255.0 + 0.5) as u8;
    f32::from(byte) / 255.0
}

/// A backdrop colour setter's four channels; a missing argument and an explicit nil read alike.
fn backdrop_color(lua: &Lua, args: mlua::MultiValue) -> [f32; 4] {
    let a: Vec<Value> = args.into_iter().collect();
    [
        backdrop_channel(lua, a.first().cloned(), false),
        backdrop_channel(lua, a.get(1).cloned(), false),
        backdrop_channel(lua, a.get(2).cloned(), false),
        backdrop_channel(lua, a.get(3).cloned(), true),
    ]
}
