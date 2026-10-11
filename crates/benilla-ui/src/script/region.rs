//! The region side of the object model ([`super::object`] is the frame side): the region wrapper
//! cache, the Region, Texture, FontString and title-region method tables, and the portrait globals.

use mlua::{Lua, MultiValue, Table, Value};

use super::object::anchor_args::{parse_set_point, resolve_rel_target, UNNAMED};
use super::object::{decode_id, id_to_lud, NamedTarget};
use super::region_map::{set_shared, Side};
use super::{
    Model, REG_FONTSTRING_META, REG_FONTSTRING_METHODS, REG_REGION_META, REG_REGION_METHODS,
    REG_TEXTURE_META, REG_TEXTURE_METHODS, REG_TITLE_META, REG_TITLE_METHODS, REG_WRAPPERS, SCREEN,
};
use crate::layout::{Anchor, Anchors};
use crate::widget::{RegionHandle, RegionKind};

/// Resolve `self` (a region wrapper) to its live [`RegionHandle`].
pub(super) fn region_handle_of(lua: &Lua, this: &Table) -> mlua::Result<RegionHandle> {
    let id = decode_id(this)?;
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    model
        .id_to_region
        .get(&id)
        .copied()
        .ok_or_else(|| mlua::Error::runtime("stale or invalid region handle"))
}

/// The Texture object a widget's texture setter was handed, as the setters resolve it: a table
/// that is no widget raises `Couldn't find 'this' in texture`, a widget that is no Texture
/// `Wrong object type, expected texture` (`0x781a8c`..`0x781ab1`, the same pair in every setter).
/// `method` is the name the setter's own format string carries; `who` is the receiver's global
/// name, or `<unnamed>`.
pub(super) fn texture_object_arg(
    lua: &Lua,
    this: &Table,
    arg: &Table,
    method: &str,
) -> mlua::Result<RegionHandle> {
    let who = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        let name = super::object::frame_handle_of(lua, this)
            .ok()
            .and_then(|h| model.arena.frame(h).and_then(|f| f.name.clone()));
        name.unwrap_or_else(|| "<unnamed>".to_string())
    };
    if decode_id(arg).is_err() {
        return Err(mlua::Error::runtime(format!(
            "{who}:{method}(): Couldn't find 'this' in texture"
        )));
    }
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    let rh = decode_id(arg)
        .ok()
        .and_then(|id| model.id_to_region.get(&id).copied())
        .filter(|&rh| model.arena.region(rh).map(|r| r.kind) == Some(RegionKind::Texture));
    rh.ok_or_else(|| {
        mlua::Error::runtime(format!(
            "{who}:{method}(): Wrong object type, expected texture"
        ))
    })
}

/// The client's re-link `0x77fd10(owner, layer, shown)` for a region a widget takes into a slot:
/// out of the old parent (its region-removed hook runs, `0x77fd55`) and into `owner` on `layer`,
/// the layer set whether or not the parent changed (`0x77fd9a`).
///
/// The third argument is the region's own shown bit (`+0xc4`, what `Show`/`Hide` write and
/// `IsShown` reads): leaving the old parent clears it (`0x77fd3d`) and `shown` writes it back
/// (`0x77fd85`, `0x77fdc8`, and `0x77fdd9` clears it when the parent and layer stand). Every widget
/// that takes a texture calls with 1 (the bar, thumb, highlight, ColorSelect slots, the label)
/// except a Button's state and checked slots, whose 0 is followed by the state switch writing the
/// bit for the texture that shows (`0x779037`, `0x7797ea`, `0x785538`): the caller applies those
/// writes after the link.
pub(super) fn link_region(
    model: &mut Model,
    rh: RegionHandle,
    owner: crate::widget::FrameHandle,
    layer: crate::order::DrawLayer,
    shown: bool,
) {
    if model.arena.set_region_owner(rh, Some(owner)) {
        // The owner supplies the region's scale to layout, so a re-link is a layout change.
        model.touch_layout_region(rh);
    }
    if let Some(r) = model.arena.region_mut(rh) {
        r.draw_layer = layer;
    }
    model.region_data.entry(rh).or_default().hidden = !shown;
}

/// Apply a Button state machine's writes to the regions' shown bit, the one `Show`/`Hide` write.
/// A region the machine names that has since been freed is skipped.
pub(super) fn apply_shown_writes(model: &mut Model, writes: crate::widget::ShownWrites) {
    for (rh, shown) in writes.0 {
        if model.arena.region(rh).is_some() {
            model.region_data.entry(rh).or_default().hidden = !shown;
        }
    }
}

/// A widget's one-texture slot taking `incoming` (a Texture object, or nothing to clear), as every
/// slot setter does (`0x778fd0`, `0x7831f0`, `0x789860`, `0x785330`, `0x78b580`..): the same
/// texture is a no-op, the displaced one is deleted, the incoming one is linked in on `layer`
/// (a thumb marker then loses its anchors, `0x767ed0`, since the widget places it), then it is
/// stored. `shown` is the link's third argument (see [`link_region`]). `get` and `set` read and
/// write the slot in the widget's own state. Answers whether the slot changed, for a caller whose
/// hook does more once it has.
pub(super) fn install_into_slot(
    model: &mut Model,
    owner: crate::widget::FrameHandle,
    incoming: Option<RegionHandle>,
    layer: crate::order::DrawLayer,
    shown: bool,
    clear_points: bool,
    get: impl Fn(&crate::widget::KindState) -> Option<RegionHandle>,
    set: impl Fn(&mut crate::widget::KindState, Option<RegionHandle>),
) -> mlua::Result<bool> {
    let current = model
        .arena
        .frame(owner)
        .map(|f| get(&f.kind_state))
        .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
    if incoming == current {
        return Ok(false);
    }
    if let Some(old) = current {
        free_region(model, old);
    }
    if let Some(rh) = incoming {
        link_region(model, rh, owner, layer, shown);
        if clear_points {
            layout::clear_points(model, rh);
        }
    }
    if let Some(f) = model.arena.frame_mut(owner) {
        set(&mut f.kind_state, incoming);
    }
    // The displaced region's free named its node; the incoming one re-derives with its new owner
    // and points.
    if let Some(rh) = incoming {
        model.touch_layout_region(rh);
    }
    Ok(true)
}

/// What a widget's texture setter was handed (`SetStatusBarTexture` `0x78f870`, `SetThumbTexture`
/// `0x7902b0`, `SetNormalTexture` `0x781970`, the ColorSelect four): a Texture object, a path, or
/// nil to clear.
pub(super) enum SlotArg {
    Object(RegionHandle),
    Path(String),
    Clear,
}

/// The setter's fork on its first argument. A table is a Texture object (`lua_type` 5), a string
/// or a number a path where the setter has a path leg (`lua_isstring` `0x6f3510`, then
/// `lua_tostring` `0x6f3690`, so `1` is the file `"1"`), nil (type 0) clears; anything else, no
/// argument included (type -1), raises `Usage: <name>:<usage>`. `object_method` names the setter in
/// the object leg's two raises, as each setter's own format string prints it.
pub(super) fn slot_arg(
    lua: &Lua,
    this: &Table,
    args: &MultiValue,
    usage: &str,
    object_method: &str,
    path_leg: bool,
) -> mlua::Result<SlotArg> {
    match args.front() {
        Some(Value::Table(t)) => {
            return texture_object_arg(lua, this, t, object_method).map(SlotArg::Object)
        }
        Some(Value::Nil) => return Ok(SlotArg::Clear),
        Some(v @ (Value::String(_) | Value::Number(_) | Value::Integer(_))) if path_leg => {
            if let Some(path) = lua.coerce_string(v.clone())? {
                return Ok(SlotArg::Path(path.to_string_lossy()));
            }
        }
        _ => {}
    }
    let who = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        super::object::frame_handle_of(lua, this)
            .ok()
            .and_then(|h| model.arena.frame(h).and_then(|f| f.name.clone()))
            .unwrap_or_else(|| "<unnamed>".to_string())
    };
    Err(mlua::Error::runtime(format!("Usage: {who}:{usage}")))
}

/// A fresh anchorless Texture region of `owner` on `layer`, as the client builds the texture a
/// path leg loads (`0x76fc40`) before it installs it as an object.
pub(super) fn new_texture_region(
    model: &mut Model,
    owner: crate::widget::FrameHandle,
    layer: crate::order::DrawLayer,
) -> Option<RegionHandle> {
    let rh = model
        .arena
        .create_region(owner, RegionKind::Texture, layer, 0)?;
    model.region_data.insert(rh, super::RegionData::default());
    model.touch_layout_region(rh);
    Some(rh)
}

/// `CSimpleTexture::SetTexture(path)` (`0x770200`) on a region: the file replaces a solid fill, and
/// `""` clears the texture, which `QuestLogFrame.lua:165` relies on.
pub(super) fn set_texture_path(model: &mut Model, rh: RegionHandle, path: &str) {
    let data = model.region_data.entry(rh).or_default();
    data.texture = (!path.is_empty()).then(|| path.to_string());
    data.fill = None;
    model.touch_layout_art(rh);
}

/// The loader's install of the texture a `<BarTexture>`, `<ThumbTexture>`, Button state-texture or
/// ColorSelect texture element (`tag`) built, as the widget's object, never through a Lua setter.
/// `layer` is the element's `drawLayer` for the bar and the thumb.
pub(crate) fn install_loaded_texture(
    lua: &Lua,
    this: &Table,
    tag: &str,
    texture: &Table,
    layer: Option<&str>,
) -> mlua::Result<()> {
    let h = super::object::frame_handle_of(lua, this)?;
    let rh = texture_object_arg(lua, this, texture, "SetStatusBarTexture")?;
    let layer = layer.and_then(super::object::draw_layer_from_str);
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    match tag {
        "BarTexture" => super::statusbar::install_loaded(&mut model, h, rh, layer),
        "ThumbTexture" => super::slider::install_loaded(&mut model, h, rh, layer),
        "NormalTexture"
        | "PushedTexture"
        | "DisabledTexture"
        | "HighlightTexture"
        | "CheckedTexture"
        | "DisabledCheckedTexture" => super::button::install_loaded(&mut model, h, tag, rh),
        _ => super::colorselect::install_loaded(&mut model, h, tag, rh),
    }
}

/// The region's shown bit (`+0xc4`) as `IsShown` reads it (`0x79b9b0`): the one bit `Show`/`Hide`
/// write, and a Button's state switch writes for its state and checked textures.
fn region_shown(model: &Model, rh: RegionHandle) -> bool {
    !model.region_data.get(&rh).is_some_and(|d| d.hidden)
}

/// Argument 1 of a global that takes a Texture as its `this` (`SetLootPortrait` `0x4c2b40`,
/// `SetInventoryPortaitTexture` `0x4c9150`), with the member prologue's three raises: a non-table
/// (`0x847ef8`), a table with no object at `[0]` (`0x847ec0`), and any object but a Texture, the
/// `IsA` test against the Texture class token `[0xcf4cdc]` (`0x847e98`).
pub(super) fn this_texture(lua: &Lua, this: &Value) -> mlua::Result<RegionHandle> {
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
    let rh = region_handle_of(lua, t).map_err(|_| wrong())?;
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    if model.arena.region(rh).map(|r| r.kind) != Some(RegionKind::Texture) {
        return Err(wrong());
    }
    Ok(rh)
}

/// `CSimpleTexture::SetTexture(NULL)` (`0x770300` with 0): the texture slot emptied, a file, a
/// solid fill and a baked unit portrait alike; the shader slot, and so the desaturation, stays.
pub(super) fn clear_texture(model: &mut Model, rh: RegionHandle) {
    let data = model.region_data.entry(rh).or_default();
    data.texture = None;
    data.fill = None;
    data.portrait_unit = None;
    model.touch_layout_art(rh);
}

/// Apply the font parts an XML element supplies (`font=`, `<FontHeight>`, `outline=`), any of
/// which may be absent. Not the `SetFont` binding: the reference applies XML fonts in `LoadXML`,
/// and its `SetFont` requires a path and a height (`0x87c69c`). Each part is an explicit set, so it
/// survives a change to the inherited font object; an empty `font=` keeps the inherited face.
pub(crate) fn apply_font_parts(
    lua: &Lua,
    this: &Table,
    path: Option<String>,
    height: Option<f32>,
    flags: Option<String>,
) -> mlua::Result<()> {
    let rh = region_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model");
    let d = model.region_data.entry(rh).or_default();
    if let Some(p) = path.filter(|p| !p.is_empty()) {
        d.font_path = Some(p);
        d.font_explicit.face = true;
    }
    if let Some(h) = height {
        d.font_height = Some(h);
        d.font_explicit.height = true;
    }
    if let Some(f) = flags {
        d.outline = super::Outline::flags(&f);
        d.font_explicit.outline = true;
    }
    model.touch_measure(rh);
    Ok(())
}

/// Get or create the wrapper table for a region id, with its kind's metatable.
pub(super) fn region_wrapper(lua: &Lua, id: u32) -> mlua::Result<Table> {
    let wrappers: Table = lua.named_registry_value(REG_WRAPPERS)?;
    if let Value::Table(t) = wrappers.get::<Value>(id)? {
        return Ok(t);
    }
    let t = lua.create_table()?;
    t.raw_set(0, Value::LightUserData(id_to_lud(id)))?;
    // The metatable is chosen once, here, not per call: a title region
    // (`CreateTitleRegion 0x773910`) answers only the 19 Region methods.
    let kind = {
        let model = lua.app_data_ref::<Model>().expect("model");
        model
            .id_to_region
            .get(&id)
            .and_then(|rh| model.arena.region(*rh))
            .map(|r| r.kind)
    };
    let meta: Table = lua.named_registry_value(match kind {
        Some(RegionKind::Texture) => REG_TEXTURE_META,
        Some(RegionKind::FontString) => REG_FONTSTRING_META,
        Some(RegionKind::Title) => REG_TITLE_META,
        // No live region: the full table, harmless since every method raises on a dead handle.
        None => REG_REGION_META,
    })?;
    t.set_metatable(Some(meta))?;
    wrappers.set(id, t.clone())?;
    Ok(t)
}

/// Install the region method tables and the region globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    install_region_methods(lua)?;

    // SetPortraitTexture(texture, unit) (`UnitFrame.lua:26`): binds the region to a unit whose
    // model the app bakes off-screen, drawn masked to its inscribed circle; a later `SetTexture`
    // or `SetPortraitToTexture` clears it.
    lua.globals().set(
        "SetPortraitTexture",
        lua.create_function(|lua, (region, unit): (Table, Value)| {
            // A non-string unit raises this exact error (`0x519fb4`, `0x8513b8`), which stock
            // `Blizzard_InspectUI.lua:49` hits on a re-open; a token that resolves to nothing only
            // blanks the portrait.
            let unit = crate::script::binding_abi::string_arg(
                lua,
                unit,
                r#"Usage: SetPortraitTexture(texture, "unit")"#,
            )?;
            let rh = region_handle_of(lua, &region)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let data = model.region_data.entry(rh).or_default();
            // Lowercased: the reference resolves the token case-insensitively (`0x519ef0` →
            // `0x515970`), stock `MerchantFrame.lua:68` passes `"NPC"`, and the app's bakes are
            // keyed lowercase.
            data.portrait_unit = Some(unit.to_ascii_lowercase());
            data.texture = None;
            data.fill = None;
            data.circular = true;
            model.touch_layout_art(rh);
            Ok(())
        })?,
    )?;

    // SetPortraitToTexture(textureName, path), an engine global: sets the texture, drawn masked
    // to its inscribed circle, and drops any unit binding. It takes the name, as both stock
    // callers pass (`ContainerFrame.lua:419`, `MailFrame.lua:174`); whether the reference also
    // takes a region object is untraced.
    lua.globals().set(
        "SetPortraitToTexture",
        lua.create_function(|lua, (name, path): (String, String)| {
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let Some(id) = model.region_names.get(&name).copied() else {
                return Ok(());
            };
            let Some(rh) = model.id_to_region.get(&id).copied() else {
                return Ok(());
            };
            let data = model.region_data.entry(rh).or_default();
            data.texture = Some(path);
            data.circular = true;
            data.portrait_unit = None;
            model.touch_layout_art(rh);
            Ok(())
        })?,
    )?;

    // `__index` is the method table itself, here and on the leaf metatables, not a function
    // (~9 ns against ~195 ns per method access); sound while nothing replaces these tables.
    let region_meta = lua.create_table()?;
    region_meta.set(
        "__index",
        lua.named_registry_value::<Table>(REG_REGION_METHODS)?,
    )?;
    lua.set_named_registry_value(REG_REGION_META, region_meta)?;
    Ok(())
}

mod layout;
mod paint;
mod text;

fn install_region_methods(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    // GetName(): the global name, or nil for an anonymous region.
    set_shared(lua, &m, Side::Region, "GetName", |lua, this: Table| {
        let id = decode_id(&this)?;
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        match region_name_of(&model, id) {
            Some(n) => Ok(Value::String(lua.create_string(&n)?)),
            None => Ok(Value::Nil),
        }
    })?;

    // ── GetObjectType / IsObjectType ─────────────────────────────────────────────────────────
    //
    // GetObjectType: a per-class string through `vtable[+0x1c]` (Texture `0x773480`, FontString
    // `0x7735d0`), one value, extra arguments ignored.
    set_shared(
        lua,
        &m,
        Side::Region,
        "GetObjectType",
        |lua, this: Table| {
            let rh = region_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(region_type_name(&model, rh))
        },
    )?;

    // IsObjectType(name) (`0x7a1290`): a case-insensitive whole-string match (`0x64a4c0`) against
    // the leaf or "Region" only, as 1.12 has no LayoutFrame, ScriptObject or Object type; a hit is
    // the number 1, a miss nil. A number is stringified and compared; anything else raises the
    // usage error, naming the region or `<unnamed>`.
    set_shared(
        lua,
        &m,
        Side::Region,
        "IsObjectType",
        |lua, (this, want): (Table, Value)| {
            let rh = region_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            let want = match &want {
                Value::String(s) => s.to_str()?.to_string(),
                Value::Number(n) => n.to_string(),
                Value::Integer(i) => i.to_string(),
                _ => {
                    let who = region_name_of(&model, decode_id(&this)?)
                        .unwrap_or_else(|| "<unnamed>".to_string());
                    return Err(mlua::Error::runtime(format!(
                        "Usage: {who}:IsObjectType(\"TYPE\")"
                    )));
                }
            };
            let leaf = region_type_name(&model, rh);
            let hit = want.eq_ignore_ascii_case(leaf) || want.eq_ignore_ascii_case("Region");
            Ok(if hit { Value::Number(1.0) } else { Value::Nil })
        },
    )?;

    // SetParent(frame) lives in the Region table (`0x7a1550`), which Texture and FontString
    // lookups fall back to (`0x79c650`, `0x79ee50`); `FuBar_FuXPFu.lua:210` calls it on a texture.
    // A non-frame raises (`0x7a16ea`, `0x87cb78`); no argument raises (`0x87cb48`) where nil
    // detaches, hence the `MultiValue`. Anchors are untouched, and nothing is returned.
    set_shared(
        lua,
        &m,
        Side::Region,
        "SetParent",
        |lua, args: mlua::MultiValue| {
            let mut it = args.into_iter();
            let Some(Value::Table(this)) = it.next() else {
                return Err(mlua::Error::runtime("SetParent: expected a region"));
            };
            let rh = region_handle_of(lua, &this)?;
            let Some(parent) = it.next() else {
                return Err(mlua::Error::runtime(
                    "SetParent(): Couldn't find region named '' (no argument)",
                ));
            };
            let wrong_type =
                || mlua::Error::runtime("SetParent(): Wrong parent object type, expected frame");
            // `_G[name]` is read before the model borrow, with no `$parent` expansion (`0x76c760`).
            let named = match &parent {
                Value::String(s) => Some(match s.to_str() {
                    Ok(n) => super::object::prefetch_named_target(lua, n.as_ref(), None),
                    Err(_) => NamedTarget::unreadable(),
                }),
                _ => None,
            };
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let new_owner = match &parent {
                Value::Nil => None,
                // A region or font-object wrapper is no frame: the same "expected frame" raise.
                Value::Table(t) => Some(
                    decode_id(t)
                        .ok()
                        .and_then(|id| model.id_to_frame.get(&id).copied())
                        .ok_or_else(wrong_type)?,
                ),
                // A name resolves through `_G[name]` and the Frame tag check (`[0xcf0c10]`).
                Value::String(_) => {
                    let nt = named.as_ref().expect("a String argument is prefetched");
                    let hit = super::object::resolve_named_target(&model, nt)
                        .and_then(|id| model.id_to_frame.get(&id).copied());
                    Some(hit.ok_or_else(|| {
                        mlua::Error::runtime(format!(
                            "SetParent(): Couldn't find region named '{}'",
                            nt.name
                        ))
                    })?)
                }
                _ => return Err(wrong_type()),
            };
            if model.arena.set_region_owner(rh, new_owner) {
                // The owner supplies the region's scale to layout, so a re-link is a layout change.
                model.touch_layout_region(rh);
            }
            Ok(())
        },
    )?;

    // A hidden region draws nothing; `IsVisible` also needs the owner frame visible.
    m.set(
        "Show",
        lua.create_function(|lua, this: Table| {
            let rh = region_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            model.region_data.entry(rh).or_default().hidden = false;
            Ok(())
        })?,
    )?;

    m.set(
        "Hide",
        lua.create_function(|lua, this: Table| {
            let rh = region_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            model.region_data.entry(rh).or_default().hidden = true;
            Ok(())
        })?,
    )?;

    m.set(
        "IsShown",
        lua.create_function(|lua, this: Table| {
            let rh = region_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            let shown = region_shown(&model, rh);
            Ok(if shown { Value::Integer(1) } else { Value::Nil })
        })?,
    )?;

    m.set(
        "IsVisible",
        lua.create_function(|lua, this: Table| {
            let rh = region_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            let shown = region_shown(&model, rh);
            let owner_visible = model
                .arena
                .region(rh)
                .and_then(|r| model.arena.frame(r.owner))
                .is_some_and(|f| f.effective_visible);
            Ok(if shown && owner_visible {
                Value::Integer(1)
            } else {
                Value::Nil
            })
        })?,
    )?;

    // In any order: each only adds to the same method table.
    paint::install(lua, &m)?;
    text::install(lua, &m)?;
    layout::install(lua, &m)?;

    // ── The title region's table ─────────────────────────────────────────────────────────────
    //
    // Exactly the Region map, copied from the full table so both share one implementation; the
    // reference's title region has no `Show`, `Hide` or texture methods.
    let title = lua.create_table()?;
    for name in super::REGION_MAP_METHODS {
        let f: Value = m.get(name)?;
        title.set(name, f)?;
    }
    // ── The leaf tables ──────────────────────────────────────────────────────────────────────
    //
    // Texture's map (`0x87c128`, 22 entries, lookup `0x79c620`) and FontString's (`0xcf5400`, 32,
    // lookup `0x79ee20`) each fall back to the Region map and nothing else. Copied from the full
    // table; a name both leaves list (one `const char*` in the client, e.g. `GetDrawLayer` at
    // `0x87c41c`) goes into both.
    for (key, extra) in [
        (REG_TEXTURE_METHODS, &super::TEXTURE_ONLY_METHODS[..]),
        (REG_FONTSTRING_METHODS, &super::FONTSTRING_ONLY_METHODS[..]),
    ] {
        let leaf = lua.create_table()?;
        for name in super::REGION_MAP_METHODS
            .iter()
            .chain(super::REGION_LEAF_SHARED.iter())
            .chain(extra.iter())
        {
            let f: Value = m.get(*name)?;
            leaf.set(*name, f)?;
        }
        lua.set_named_registry_value(key, leaf)?;
    }
    for (meta_key, methods_key) in [
        (REG_TEXTURE_META, REG_TEXTURE_METHODS),
        (REG_FONTSTRING_META, REG_FONTSTRING_METHODS),
    ] {
        let meta = lua.create_table()?;
        meta.set("__index", lua.named_registry_value::<Table>(methods_key)?)?;
        lua.set_named_registry_value(meta_key, meta)?;
    }

    let title_meta = lua.create_table()?;
    title_meta.set("__index", title.clone())?;
    lua.set_named_registry_value(REG_TITLE_METHODS, title)?;
    lua.set_named_registry_value(REG_TITLE_META, title_meta)?;

    lua.set_named_registry_value(REG_REGION_METHODS, m)?;
    Ok(())
}

/// This region's global name, or `None` when anonymous: a scan of the region-name registry, the
/// one authority, linear in named regions.
pub(super) fn region_name_of(model: &Model, id: u32) -> Option<String> {
    model
        .region_names
        .iter()
        .find(|&(_, &v)| v == id)
        .map(|(k, _)| k.clone())
}

/// Publish a region's name into the registry a named `relativeTo` resolves through, first name
/// winning as for frames. For sub-textures a setter creates and the XML loader names later
/// (`SetThumbTexture`, `SetColorWheelTexture`); `CreateTexture(name)` publishes its own.
pub(crate) fn publish_region_name(lua: &Lua, name: &str, region: &Table) {
    let Ok(id) = super::object::decode_id(region) else {
        return;
    };
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    model.region_names.entry(name.to_string()).or_insert(id);
}

/// The type `GetObjectType()` answers and `IsObjectType` matches; `"Region"` for a dead handle.
pub(super) fn region_type_name(model: &Model, rh: RegionHandle) -> &'static str {
    match model.arena.region(rh).map(|r| r.kind) {
        Some(RegionKind::Texture) => "Texture",
        Some(RegionKind::FontString) => "FontString",
        // A title region is a bare Region (`CreateTitleRegion 0x773910`).
        Some(RegionKind::Title) => "Region",
        None => "Region",
    }
}

/// Free a region and every trace of its identity (arena slot, paint, rect, name, id), so a held
/// wrapper raises as stale; the reference frees too, as `CSimpleButton::SetFontString 0x778d20`
/// deletes the label it replaces.
pub(crate) fn free_region(model: &mut Model, rh: RegionHandle) {
    model.region_data.remove(&rh);
    model.region_resolved.remove(&rh);
    if let Some(id) = model.region_to_id.remove(&rh) {
        model.id_to_region.remove(&id);
        model.region_names.retain(|_, v| *v != id);
        // Its node leaves the cached graph with its edges at the next resolve, and whatever
        // anchored to it re-resolves without it.
        model.name_layout_node(id);
    }
    model.arena.destroy_region(rh);
}

pub(super) fn region_owner_id(model: &mut Model, rh: RegionHandle) -> u32 {
    match model.arena.region(rh).map(|r| r.owner) {
        Some(owner) => model.frame_id(owner),
        None => SCREEN,
    }
}

/// The reference's implicit anchor for a region with a parent and no anchors, run after its XML
/// loads (texture `0x7701c0`, fontstring `0x771480`), and from Lua `CreateTexture` or
/// `CreateFontString` only on a template hit. A Texture gets `SetAllPoints(parent)`, so an
/// authored `<Size>` goes unread; a FontString gets one middle-row point picked by its justify
/// word (`[+0x120]`, our [`RegionData::justify`]) and keeps its size; a title region gets none.
/// They are ordinary anchors after: re-applying authored ones needs `ClearAllPoints` first.
pub(crate) fn implicit_creation_anchor(model: &mut Model, rh: RegionHandle) {
    let Some((kind, owner)) = model.arena.region(rh).map(|r| (r.kind, r.owner)) else {
        return;
    };
    let owner_id = model.frame_id(owner);
    let data = model.region_data.entry(rh).or_default();
    if !data.anchors.is_empty() {
        return;
    }
    use crate::layout::Point;
    match kind {
        RegionKind::Texture => {
            data.anchors = Anchors::from([
                Anchor::new(Point::TopLeft, owner_id, Point::TopLeft, 0.0, 0.0),
                Anchor::new(Point::BottomRight, owner_id, Point::BottomRight, 0.0, 0.0),
            ]);
        }
        RegionKind::FontString => {
            let point = justify_anchor_point(data.justify.0);
            data.anchors = Anchors::from([Anchor::new(point, owner_id, point, 0.0, 0.0)]);
        }
        RegionKind::Title => return,
    }
    model.touch_layout_region(rh);
}

/// The middle-row point a justify word selects, as the FontString post-step (`0x771480`) and the
/// button label adopter (`0x778d20`) both compute it: LEFT for 1, RIGHT for 4, else CENTER.
pub(crate) fn justify_anchor_point(word: u32) -> crate::layout::Point {
    use crate::layout::Point;
    match word & crate::justify::H_MASK {
        0x01 => Point::Left,
        0x04 => Point::Right,
        _ => Point::Center,
    }
}

/// Anchor `point` to the owner's same point at (0, 0), only when the region has no anchor at all.
pub(crate) fn anchor_unanchored_at(
    model: &mut Model,
    rh: RegionHandle,
    point: crate::layout::Point,
) {
    let Some(owner) = model.arena.region(rh).map(|r| r.owner) else {
        return;
    };
    let owner_id = model.frame_id(owner);
    let data = model.region_data.entry(rh).or_default();
    if !data.anchors.is_empty() {
        return;
    }
    data.anchors = Anchors::from([Anchor::new(point, owner_id, point, 0.0, 0.0)]);
    model.touch_layout_region(rh);
}

/// [`implicit_creation_anchor`] for the loader, which holds region wrappers.
pub(crate) fn implicit_creation_anchor_lua(lua: &Lua, wrapper: &Table) -> mlua::Result<()> {
    let rh = region_handle_of(lua, wrapper)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    implicit_creation_anchor(&mut model, rh);
    Ok(())
}

/// The receiver's name for error strings and what a leading `$parent` expands to, the owner frame
/// (`+0x9c`), read under a borrow dropped before the ladder reads `_G`.
pub(super) fn region_ladder_context(lua: &Lua, rh: RegionHandle) -> (String, String) {
    let mut model = lua.app_data_mut::<Model>().expect("model");
    let id = model.region_id(rh);
    let who = model
        .region_names
        .iter()
        .find(|(_, &v)| v == id)
        .map(|(n, _)| n.clone())
        .unwrap_or_else(|| UNNAMED.to_string());
    let owner = model.arena.region(rh).map(|r| r.owner);
    let base = super::object::parent_token_base(&model, owner);
    (who, base)
}

/// Bit-exact size equality, the layout gate's own test, so a setter's no-op check agrees with it.
pub(super) fn size_bits_eq(a: Option<(f32, f32)>, b: Option<(f32, f32)>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some((aw, ah)), Some((bw, bh))) => {
            aw.to_bits() == bw.to_bits() && ah.to_bits() == bh.to_bits()
        }
        _ => false,
    }
}

/// `Region:SetPoint`, the same binding as the frame's (`0x7a2540`, on `CScriptRegion`): the shared
/// argument ladder ([`super::object::anchor_args`]), with the owner as layout parent.
pub(super) fn region_set_point(lua: &Lua, this: &Table, args: &MultiValue) -> mlua::Result<()> {
    let rh = region_handle_of(lua, this)?;
    let (who, base) = region_ladder_context(lua, rh);
    // No model borrow is alive across the ladder's `_G` read.
    let p = parse_set_point(lua, args, &who, &base)?;

    let mut model = lua.app_data_mut::<Model>().expect("model");
    let me = model.region_id(rh);
    let owner = region_owner_id(&mut model, rh);
    let rel_to_id = resolve_rel_target(&model, &p.target, &who, "SetPoint", me, owner)?;
    let (point, rel_point, x, y) = (p.point, p.rel_point, p.x, p.y);

    // The frame's store and its unchanged-anchor test (`0x767c70`).
    let units = model.layout_units();
    let data = model.region_data.entry(rh).or_default();
    if data
        .anchors
        .set(Anchor::new(point, rel_to_id, rel_point, x, y), units)
    {
        model.touch_layout_region(rh);
    }
    Ok(())
}

/// `Region:GetWidth()`/`GetHeight()` (`0x7a1e00`, `0x7a2030`): the geometry-vtable call the rect
/// resolver also makes (`0x767579`), so the Lua size is the rect's. Per axis, a Texture answers
/// its authored size unless exactly 0, else its texel extent (`0x770720`, `0x770790`). A
/// FontString answers its authored size unless 0 (`0x77294a`), else its measure, either floored
/// at one unit (`0x772930`, `0x772a60`); the width is unwrapped, as `GetStringWidth` gives it
/// (`0x772890`, `0x79e510`), the height wrapped (`0x7729b0`). A title region answers its field
/// (`0x768420`, `0x768410`).
pub(super) fn measured_wh(lua: &Lua, this: &Table) -> mlua::Result<(f32, f32)> {
    let rh = region_handle_of(lua, this)?;
    // Measures this tick when a host font engine is installed; a no-op for a Texture or a
    // current measure.
    super::measure::ensure_measured(lua, rh);
    let model = lua.app_data_ref::<Model>().expect("model");
    Ok(virtual_span(&model, rh))
}

/// [`measured_wh`]'s body on a borrowed model, for the engine's own callers.
pub(super) fn virtual_span(model: &Model, rh: RegionHandle) -> (f32, f32) {
    let Some(d) = model.region_data.get(&rh) else {
        return (0.0, 0.0);
    };
    let (aw, ah) = d.size.unwrap_or((0.0, 0.0));
    match model.arena.region(rh).map(|r| r.kind) {
        Some(RegionKind::FontString) => {
            // The measure key carries the owner's effective scale, as the request loop stamps it.
            // Unlike the layout sweep, which keeps the last box, the getter refuses a stale
            // measure rather than answer with the previous string's.
            let scale = model
                .arena
                .region(rh)
                .and_then(|r| model.arena.frame(r.owner))
                .map(|f| f.effective_scale)
                .unwrap_or(1.0);
            let m = d.measured.filter(|m| m.key == d.measure_key(scale));
            // The floor applies to a known extent. Empty text is a known zero and floors; a pending
            // measure, possible only in a VM with no measurer installed, answers 0, where the
            // reference measures inline and always knows.
            let floor = super::layout::FONTSTRING_MIN_SPAN;
            let known = |from_measure: fn(&crate::script::types::MeasuredText) -> f32| {
                if d.text.as_deref().is_none_or(str::is_empty) {
                    Some(0.0)
                } else {
                    m.as_ref().map(from_measure)
                }
            };
            let w = if aw != 0.0 {
                aw.max(floor)
            } else {
                known(|m| m.natural_w).map_or(0.0, |v| v.max(floor))
            };
            let h = if ah != 0.0 {
                ah.max(floor)
            } else {
                known(|m| m.h).map_or(0.0, |v| v.max(floor))
            };
            (w, h)
        }
        Some(RegionKind::Texture) => {
            let texel = (aw == 0.0 || ah == 0.0)
                .then(|| super::layout::content_span(d, model.texture_size_probe.as_ref()))
                .flatten();
            (
                if aw != 0.0 {
                    aw
                } else {
                    texel.map_or(0.0, |t| t.0)
                },
                if ah != 0.0 {
                    ah
                } else {
                    texel.map_or(0.0, |t| t.1)
                },
            )
        }
        // A title region: the flat field, like a frame (vtable `0x81c9c8`, `+0x1c` = `0x768420`).
        _ => (aw, ah),
    }
}

/// [`virtual_span`] on screen: the region's own units times its owner's effective scale, as the
/// layout sizes a region (`0x76ac90`). For the parts the engine seats itself, with no anchors: a
/// slider's thumb, a colour picker's markers.
pub(super) fn screen_span(model: &Model, rh: RegionHandle) -> (f32, f32) {
    let (w, h) = virtual_span(model, rh);
    let s = model
        .arena
        .region(rh)
        .map_or(1.0, |r| super::object::eff_scale(model, r.owner));
    (w * s, h * s)
}
