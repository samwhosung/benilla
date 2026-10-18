//! Frame methods: event registration, script handlers, drag registration and region creation.

use std::collections::HashSet;

use mlua::{Function, Lua, MultiValue, Table, Value};

use crate::script::binding_abi::optional_string;
use crate::script::region::region_wrapper;
use crate::script::{Model, RegionData, REG_SCRIPTS, SCRIPT_KINDS};
use crate::widget::{FrameHandle, RegionKind};

use super::{decode_id, draw_layer_from_str, frame_handle_of, publish_global};

/// Append `h` to `event`'s listener list unless it is there (`0x7021a8` → `0x702264`): the walk
/// (`0x703e50`) follows registration order, and a re-register keeps its place and never fires twice.
fn register(model: &mut Model, h: FrameHandle, event: &'static str) {
    if !model.event_to_frames.contains_key(event) {
        model.event_to_frames.insert(event.to_string(), Vec::new());
    }
    let list = model
        .event_to_frames
        .get_mut(event)
        .expect("list just made");
    if list.contains(&h) {
        return;
    }
    list.push(h);
    model
        .frame_events
        .entry(h)
        .or_default()
        .insert(event.to_string());
}

/// Populate `m`'s event, script and region-creation methods.
pub(super) fn install(lua: &Lua, m: &Table) -> mlua::Result<()> {
    // Events + scripts
    m.set(
        "RegisterEvent",
        lua.create_function(|lua, (this, event): (Table, String)| {
            let h = frame_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            // A name the VM's event table lacks registers nothing and raises nothing (`0x702140`).
            let Some(event) = model.vm.event_key(&event) else {
                return Ok(());
            };
            register(&mut model, h, event);
            Ok(())
        })?,
    )?;
    m.set(
        "UnregisterEvent",
        lua.create_function(|lua, (this, event): (Table, String)| {
            let h = frame_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            // Matched as `RegisterEvent` matches (`0x702280`).
            let Some(event) = model.vm.event_key(&event).map(str::to_string) else {
                return Ok(());
            };
            if let Some(list) = model.event_to_frames.get_mut(&event) {
                list.retain(|x| x != &h);
            }
            if let Some(set) = model.frame_events.get_mut(&h) {
                set.remove(&event);
            }
            Ok(())
        })?,
    )?;
    // `RegisterAllEvents()` (`0x774c20` → `0x7023e0`): appends the frame to every event's list
    // where it is absent (scan `0x702428`, skip `0x7024c7`, append `0x7052d0` at `0x702440`), as
    // a `RegisterEvent` per event would; there is no all-events flag.
    m.set(
        "RegisterAllEvents",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            for &event in model.vm.events() {
                register(&mut model, h, event);
            }
            Ok(())
        })?,
    )?;
    // `UnregisterAllEvents()`: drops every registration the frame holds.
    m.set(
        "UnregisterAllEvents",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let events = model.frame_events.remove(&h).unwrap_or_default();
            for event in events {
                if let Some(list) = model.event_to_frames.get_mut(&event) {
                    list.retain(|x| x != &h);
                }
            }
            Ok(())
        })?,
    )?;
    m.set(
        "SetScript",
        lua.create_function(
            |lua, (this, name, func): (Table, String, Option<Function>)| {
                set_script(lua, &this, &name, func)
            },
        )?,
    )?;
    m.set(
        "GetScript",
        lua.create_function(|lua, (this, name): (Table, String)| get_script(lua, &this, &name))?,
    )?;
    // `HasScript(name)`: 1 if the widget can carry that script kind, else nil, whether or not one
    // is set. Answered from the flat `SCRIPT_KINDS`, so another type's kind reads 1 too, where the
    // reference's tables are per widget type (base `0x76a0d0`; a plain Frame has no `OnClick`).
    m.set(
        "HasScript",
        lua.create_function(|_, (_this, name): (Table, String)| {
            Ok(crate::script::binding_abi::flag(
                SCRIPT_KINDS.iter().any(|k| k.eq_ignore_ascii_case(&name)),
            ))
        })?,
    )?;
    // `RegisterForDrag(button, ...)` (`0x776d60`): on the shared table, since any frame can be a
    // drag source; each call replaces the set (none clears it), matched case-insensitively.
    m.set(
        "RegisterForDrag",
        lua.create_function(|lua, (this, args): (Table, MultiValue)| {
            let h = frame_handle_of(lua, &this)?;
            let mut set = HashSet::new();
            for v in args.iter() {
                if let Value::String(s) = v {
                    set.insert(s.to_str()?.to_string());
                }
            }
            let mut model = lua.app_data_mut::<Model>().expect("model");
            model.drag_registered.insert(h, set);
            Ok(())
        })?,
    )?;
    // Regions
    // `CreateTexture(name, layer, inherits)` (`0x773a20`).
    m.set(
        // `name` and `layer` pass `lua_isstring` (`0x6f3510`, then `0x6f3690`) with the result
        // unchecked, so a table is absent and a number is stringified: no argument type raises.
        "CreateTexture",
        lua.create_function(
            |lua, (this, name, layer, inherits): (Table, Value, Value, Value)| {
                let name = optional_string(lua, &name);
                let layer = optional_string(lua, &layer);
                let wrapper = create_region(lua, &this, RegionKind::Texture, name, layer)?;
                if let Some(from) = strict_string_arg(&inherits).filter(|s| !s.is_empty()) {
                    apply_region_inherits(lua, &wrapper, &from)?;
                }
                Ok(wrapper)
            },
        )?,
    )?;
    // ── Title region: CreateTitleRegion / GetTitleRegion ──
    //
    // `CreateTitleRegion` (`0x773910`) reads no argument and keeps one region per frame
    // (`CSimpleFrame+0xA8`): a second call clears that region's anchors and returns it, and a new
    // one has none. The reference's region answers only the 19 Region methods (`0x7a2ea0`); ours
    // shares the region metatable, so it also accepts the texture methods, inertly.
    m.set(
        "CreateTitleRegion",
        lua.create_function(|lua, this: Table| {
            let owner = frame_handle_of(lua, &this)?;
            let existing = {
                let model = lua.app_data_ref::<Model>().expect("model");
                model.arena.frame(owner).and_then(|f| f.title_region)
            };
            if let Some(rh) = existing {
                let id = {
                    let mut model = lua.app_data_mut::<Model>().expect("model");
                    let d = model.region_data.entry(rh).or_default();
                    let changed = !d.anchors.is_empty();
                    d.anchors.clear();
                    if changed {
                        model.touch_layout_region(rh);
                    }
                    model.region_id(rh)
                };
                return region_wrapper(lua, id);
            }
            let wrapper = create_region(lua, &this, RegionKind::Title, None, None)?;
            let id = decode_id(&wrapper)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let rh = *model
                .id_to_region
                .get(&id)
                .expect("the region we just created is registered");
            if let Some(f) = model.arena.frame_mut(owner) {
                f.title_region = Some(rh);
            }
            Ok(wrapper)
        })?,
    )?;
    m.set(
        "GetTitleRegion",
        lua.create_function(|lua, this: Table| {
            let owner = frame_handle_of(lua, &this)?;
            let found = {
                let mut model = lua.app_data_mut::<Model>().expect("model");
                model
                    .arena
                    .frame(owner)
                    .and_then(|f| f.title_region)
                    .map(|rh| model.region_id(rh))
            };
            match found {
                Some(id) => Ok(Value::Table(region_wrapper(lua, id)?)),
                // One nil value (`0x773820`), where `GetBackdrop` answers none.
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // `CreateFontString(name, layer, inherits)` (`0x773c30`), with the same argument rules.
    m.set(
        "CreateFontString",
        lua.create_function(
            |lua, (this, name, layer, inherits): (Table, Value, Value, Value)| {
                let name = optional_string(lua, &name);
                let layer = optional_string(lua, &layer);
                let wrapper = create_region(lua, &this, RegionKind::FontString, name, layer)?;
                if let Some(from) = strict_string_arg(&inherits).filter(|s| !s.is_empty()) {
                    apply_region_inherits(lua, &wrapper, &from)?;
                }
                Ok(wrapper)
            },
        )?,
    )?;

    Ok(())
}

/// `SetScript(name, func)`: stores the closure under one of [`SCRIPT_KINDS`], else raises.
/// Deviation: real 1.12 kinds nothing fires yet raise too (`OnMessageScrollChanged`,
/// `OnInputLanguageChanged`), because an accepted handler that never runs fails silently. The movie
/// frame's three are accepted all the same: the stock XML sets them, and its player ([`super::super::movieframe`])
/// is the one thing that fires them. `OnAttributeChanged` is 2.0's, with no 1.12 slot.
fn set_script(lua: &Lua, this: &Table, name: &str, func: Option<Function>) -> mlua::Result<()> {
    let kind = SCRIPT_KINDS
        .iter()
        .copied()
        .find(|&k| k.eq_ignore_ascii_case(name))
        .ok_or_else(|| mlua::Error::runtime(format!("SetScript: unsupported script '{name}'")))?;
    let h = frame_handle_of(lua, this)?;
    let id = lua.app_data_mut::<Model>().expect("model").frame_id(h);

    // The closure lives Lua-side in `REG_SCRIPTS[id][kind]`; Rust keeps a presence mirror.
    let scripts: Table = lua.named_registry_value(REG_SCRIPTS)?;
    let per: Table = match scripts.get::<Value>(id)? {
        Value::Table(t) => t,
        _ => {
            let t = lua.create_table()?;
            scripts.set(id, t.clone())?;
            t
        }
    };
    match func {
        Some(f) => {
            per.set(kind, f)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            // The first handler of a kind joins that kind's list; nothing else writes `scripts`.
            if model.scripts.entry(h).or_default().insert(kind) {
                match kind {
                    "OnUpdate" => {
                        model.on_update_frames.push(h);
                        model.on_update_joins += 1;
                    }
                    "OnSizeChanged" => model.on_size_changed_frames.push(h),
                    "OnUpdateModel" => model.on_update_model_frames.push(h),
                    _ => {}
                }
            }
        }
        None => {
            per.set(kind, Value::Nil)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            if let Some(set) = model.scripts.get_mut(&h) {
                if set.remove(&kind) {
                    match kind {
                        "OnUpdate" => model.on_update_frames.retain(|&x| x != h),
                        "OnSizeChanged" => {
                            model.on_size_changed_frames.retain(|&x| x != h);
                        }
                        "OnUpdateModel" => model.on_update_model_frames.retain(|&x| x != h),
                        _ => {}
                    }
                }
            }
        }
    }
    Ok(())
}

fn get_script(lua: &Lua, this: &Table, name: &str) -> mlua::Result<Value> {
    let kind = match SCRIPT_KINDS
        .iter()
        .copied()
        .find(|&k| k.eq_ignore_ascii_case(name))
    {
        Some(k) => k,
        None => return Ok(Value::Nil),
    };
    let id = decode_id(this)?;
    let scripts: Table = lua.named_registry_value(REG_SCRIPTS)?;
    match scripts.get::<Value>(id)? {
        Value::Table(t) => t.get::<Value>(kind),
        _ => Ok(Value::Nil),
    }
}

/// Apply the `inherits` argument: the font-object registry first (`0x773d39`), then the template
/// registry (`0x773d47`); a name in neither raises (`0x87957c` / `0x879544`). A template that is
/// not a font object only warns: the reference applies the template, which is not built here.
fn apply_region_inherits(lua: &Lua, wrapper: &Table, from: &str) -> mlua::Result<()> {
    use mlua::ObjectLike;
    // Through `SetFontObject`, the binding the XML `inherits=` path uses.
    if wrapper.call_method::<()>("SetFontObject", from).is_ok() {
        return Ok(());
    }
    let is_template = lua
        .app_data_ref::<Model>()
        .expect("model")
        .framexml_templates
        .borrow()
        .contains(from);
    if is_template {
        let mut model = lua.app_data_mut::<Model>().expect("model");
        model.record_warning(format!(
            "CreateTexture/CreateFontString: '{from}' is a registered TEMPLATE, not a font object; \
             the region is created but the template's content is not applied (no corpus caller \
             does this)"
        ));
        return Ok(());
    }
    Err(mlua::Error::runtime(format!(
        "Couldn't find inherited node \"{from}\""
    )))
}

/// The region constructors' strict `lua_type == LUA_TSTRING` gate on `inherits` (`0x773d28`
/// `CreateFontString`, `0x773b06` `CreateTexture`): unlike [`optional_string`], a number is
/// ignored, where `CreateFrame` stringifies it first and looks it up as a name.
fn strict_string_arg(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => s.to_str().ok().map(|s| s.to_owned()),
        _ => None,
    }
}

fn create_region(
    lua: &Lua,
    this: &Table,
    kind: RegionKind,
    name: Option<String>,
    layer: Option<String>,
) -> mlua::Result<Table> {
    let owner = frame_handle_of(lua, this)?;
    let dl = layer
        .as_deref()
        .and_then(draw_layer_from_str)
        .unwrap_or_default();

    let id = {
        let mut model = lua.app_data_mut::<Model>().expect("model");
        let rh = model
            .arena
            .create_region(owner, kind, dl, 0)
            .ok_or_else(|| mlua::Error::runtime("CreateTexture/FontString: dead owner frame"))?;
        model.region_data.insert(rh, RegionData::default());
        model.region_id(rh)
    };

    let wrapper = region_wrapper(lua, id)?;
    if let Some(name) = name {
        // A leading `$parent` expands as in XML, its walk to a named frame starting at the owner:
        // both bindings name the region through `CScriptRegion::SetName 0x76c650` (`0x773ba1`,
        // `0x773dc4`).
        let name = {
            let model = lua.app_data_ref::<Model>().expect("model");
            crate::framexml::resolve_name(&name, &super::parent_token_base(&model, Some(owner)))
        };
        publish_global(lua, &name, &wrapper)?;
        // The region keeps its own name; the lookup a sibling's `SetPoint` names it by stays
        // first-come, as `_G` is.
        let mut model = lua.app_data_mut::<Model>().expect("model");
        if let Some(rh) = model.id_to_region.get(&id).copied() {
            crate::script::region::set_region_name(&mut model, rh, id, &name);
        }
    }
    Ok(wrapper)
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    /// Both frames share both events, so clearing one must leave the other's registrations.
    #[test]
    fn unregister_all_events_clears_one_frame_and_only_that_frame() {
        let mut s = UiScript::new().unwrap();
        s.run(
            r#"
            Mine  = CreateFrame("Frame", "UnregAllMine")
            Yours = CreateFrame("Frame", "UnregAllYours")
            Seen = {}
            for _, f in ipairs({ Mine, Yours }) do
                f:RegisterEvent("PLAYER_ENTERING_WORLD")
                f:RegisterEvent("PLAYER_LOGIN")
                f:SetScript("OnEvent", function() Seen[event] = (Seen[event] or 0) + 1 end)
            end
            "#,
        )
        .unwrap();

        s.fire_event("PLAYER_LOGIN", vec![]);
        assert_eq!(s.eval::<i64>("return Seen.PLAYER_LOGIN").unwrap(), 2);

        s.run("Mine:UnregisterAllEvents()").unwrap();
        s.run("Seen = {}").unwrap();
        s.fire_event("PLAYER_LOGIN", vec![]);
        s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
        assert_eq!(
            s.eval::<i64>("return Seen.PLAYER_LOGIN").unwrap(),
            1,
            "the other frame's registration must survive — they shared the event"
        );
        assert_eq!(
            s.eval::<i64>("return Seen.PLAYER_ENTERING_WORLD").unwrap(),
            1
        );

        // Idempotent, and harmless on a frame that never registered anything.
        s.run("Mine:UnregisterAllEvents() CreateFrame(\"Frame\"):UnregisterAllEvents()")
            .unwrap();
    }
}
