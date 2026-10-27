//! Frame methods: event registration, script handlers, drag registration and region creation.

use std::collections::HashSet;

use mlua::{Function, Lua, MultiValue, Table, Value};

use crate::script::binding_abi::{method_string, optional_string, usage};
use crate::script::region::region_wrapper;
use crate::script::script_map::script_name;
use crate::script::{Model, RegionData, REG_SCRIPTS};
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
    // Events + scripts. Each verb tests argument 2 with `lua_isstring` after the receiver check
    // (RegisterEvent `0x774acc`, UnregisterEvent `0x774bbc`, HasScript `0x77470c`, GetScript
    // `0x77480c`, SetScript `0x77495c`) and raises its usage line naming the frame.
    m.set(
        "RegisterEvent",
        lua.create_function(|lua, (this, event): (Table, Value)| {
            let h = frame_handle_of(lua, &this)?;
            let event = method_string(lua, &this, event, r#"RegisterEvent("event")"#)?;
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
        lua.create_function(|lua, (this, event): (Table, Value)| {
            let h = frame_handle_of(lua, &this)?;
            let event = method_string(lua, &this, event, r#"UnregisterEvent("event")"#)?;
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
    // SetScript's argument 3 must be a function or nil by `lua_type` (`0x774975`, `0x774986`), so
    // a missing one, type -1, raises like a wrong type: `Usage: %s:SetScript("type", function)`
    // (`0x8796b4`).
    m.set(
        "SetScript",
        lua.create_function(|lua, (this, args): (Table, MultiValue)| {
            frame_handle_of(lua, &this)?;
            let line = r#"SetScript("type", function)"#;
            let mut args = args.into_iter();
            let name = method_string(lua, &this, args.next().unwrap_or(Value::Nil), line)?;
            let func = match args.next() {
                Some(Value::Function(f)) => Some(f),
                Some(Value::Nil) => None,
                _ => return Err(usage(lua, &this, line)),
            };
            set_script(lua, &this, &name, func)
        })?,
    )?;
    m.set(
        "GetScript",
        lua.create_function(|lua, (this, name): (Table, Value)| {
            frame_handle_of(lua, &this)?;
            let name = method_string(lua, &this, name, r#"GetScript("type")"#)?;
            get_script(lua, &this, &name)
        })?,
    )?;
    // `HasScript(name)` (`0x774772`): 1 if the frame's type has the name in its script map, else
    // nil, whether or not one is set.
    m.set(
        "HasScript",
        lua.create_function(|lua, (this, name): (Table, Value)| {
            let h = frame_handle_of(lua, &this)?;
            let name = method_string(lua, &this, name, r#"HasScript("type")"#)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(crate::script::binding_abi::flag(
                frame_script_name(&model, h, &name).is_some(),
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
                    apply_region_inherits(lua, &this, &wrapper, &from, "CreateTexture")?;
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
                    apply_region_inherits(lua, &this, &wrapper, &from, "CreateFontString")?;
                }
                Ok(wrapper)
            },
        )?,
    )?;

    Ok(())
}

/// `name` in the script map of frame `h`'s type ([`script_name`]).
fn frame_script_name(model: &Model, h: FrameHandle, name: &str) -> Option<&'static str> {
    script_name(model.arena.frame(h)?.kind, name)
}

/// The script `name` of frame `h`'s type, else the reference's raise (`0x879678`, at `0x7749d1` for
/// `SetScript` and `0x774884` for `GetScript`): `%s doesn't have a "%s" script`, naming the frame
/// and the name as given.
fn typed_script(lua: &Lua, this: &Table, h: FrameHandle, name: &str) -> mlua::Result<&'static str> {
    let found = {
        let model = lua.app_data_ref::<Model>().expect("model");
        frame_script_name(&model, h, name)
    };
    found.ok_or_else(|| {
        let owner = crate::script::binding_abi::name_of(lua, this);
        mlua::Error::runtime(format!("{owner} doesn't have a \"{name}\" script"))
    })
}

/// `SetScript(name, func)`: stores the closure under a name of the frame's type
/// ([`script_name`]), else raises. `OnAttributeChanged` is 2.0's, with no 1.12 slot.
fn set_script(lua: &Lua, this: &Table, name: &str, func: Option<Function>) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let kind = typed_script(lua, this, h, name)?;
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
    let kind = typed_script(lua, this, frame_handle_of(lua, this)?, name)?;
    let id = decode_id(this)?;
    let scripts: Table = lua.named_registry_value(REG_SCRIPTS)?;
    let value = match scripts.get::<Value>(id)? {
        Value::Table(t) => t.get::<Value>(kind)?,
        _ => Value::Nil,
    };
    // A slot holding a failed compile's -1 pushes nil (`0x7748b9`).
    match value {
        Value::Function(f) if f == broken_handler(lua)? => Ok(Value::Nil),
        v => Ok(v),
    }
}

/// The registry slot of [`broken_handler`].
const REG_BROKEN_HANDLER: &str = "__benilla_broken_handler";

/// What an XML handler that fails to compile leaves in its slot: `0x704c70` answers -1 (`0x704c9f`),
/// which `0x7025c0` stores (`0x702642`). Firing it calls nil, an error each time, and `GetScript`
/// answers nil ([`get_script`]). One function for every such slot.
pub(crate) fn broken_handler(lua: &Lua) -> mlua::Result<Function> {
    if let Some(f) = lua.named_registry_value::<Option<Function>>(REG_BROKEN_HANDLER)? {
        return Ok(f);
    }
    let f = lua.create_function(|_, ()| -> mlua::Result<()> {
        Err(mlua::Error::runtime("attempt to call a nil value"))
    })?;
    lua.set_named_registry_value(REG_BROKEN_HANDLER, f.clone())?;
    Ok(f)
}

/// Apply the `inherits` argument: the font-object registry first (`0x773d39`), then the template
/// registry (`0x773d47`); a name in neither raises `%s:CreateTexture(): Couldn't find inherited
/// node "%s"` (`0x879544`, `0x773b4d`) or the `CreateFontString` line (`0x87957c`), naming the
/// frame `this`. A template that is not a font object only warns: the reference applies the
/// template, which is not built here.
fn apply_region_inherits(
    lua: &Lua,
    this: &Table,
    wrapper: &Table,
    from: &str,
    verb: &str,
) -> mlua::Result<()> {
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
        "{}:{verb}(): Couldn't find inherited node \"{from}\"",
        crate::script::binding_abi::name_of(lua, this)
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

/// A `<TitleRegion>` element's own region (`0x769b3e`-`0x769b97`): unlike `CreateTitleRegion`,
/// the XML arm never reuses the frame's region. It allocates a new one, parents it to the frame
/// and stores it at `+0xa8` over whatever was there, so a template's title region and an
/// instance's are two objects and only the last one read is the frame's. The one displaced is
/// left unreferenced (the reference neither frees nor relinks it), detached here so it neither
/// hits nor lists.
pub(crate) fn xml_title_region(lua: &Lua, this: &Table) -> mlua::Result<Table> {
    let owner = frame_handle_of(lua, this)?;
    let wrapper = create_region(lua, this, RegionKind::Title, None, None)?;
    let id = decode_id(&wrapper)?;
    let mut model = lua.app_data_mut::<Model>().expect("model");
    let rh = *model
        .id_to_region
        .get(&id)
        .expect("the region we just created is registered");
    let old = model
        .arena
        .frame_mut(owner)
        .and_then(|f| f.title_region.replace(rh));
    if let Some(r) = old.and_then(|old| model.arena.region_mut(old)) {
        r.detached = true;
    }
    Ok(wrapper)
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
    // `SetName 0x76c650` refuses null or `""` (`0x76c664`), so an empty name leaves it unnamed.
    if let Some(name) = name.filter(|n| !n.is_empty()) {
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
