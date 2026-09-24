//! The FrameScript object model, frame side: `CreateFrame`, the frame wrappers and metatables, and
//! the shared frame methods; the region side is [`super::region`].
//!
//! A frame's Lua value is a table `T` with `T[0] = lightuserdata(id)` (`0x701bd0`), cached by id
//! and published to `_G` by name without overwriting; its metatable is its kind's
//! ([`frame_meta_for`]). Every table lives in the Lua registry under a named key, and Rust holds
//! none across calls (the MAXCSTACK discipline).

use std::ffi::c_void;

use mlua::{LightUserData, Lua, ObjectLike, Table, Value};

use super::binding_abi::optional_string;
use super::{Model, REG_FRAME_META, REG_FRAME_METHODS, REG_SCRIPTS, REG_WRAPPERS};
use crate::layout::Point;
use crate::order::{DrawLayer, Strata};
use crate::widget::{FrameHandle, FrameKind};

// The frame method clusters, split out for size.
pub(crate) mod anchor_args;
mod events_regions;
mod frame_state;
mod layout_methods;
pub(crate) use layout_methods::eff_scale;
pub(crate) mod movable;
pub(crate) mod toplevel;
pub(crate) use layout_methods::{anchor_bits_eq, anchor_retarget_is_structural};
pub(crate) use movable::{advance_move, advance_size, FrameMove, FrameSizing};

// ─────────────────────────────────────────────────────────────────────────────────────────────
// id ↔ lightuserdata
// ─────────────────────────────────────────────────────────────────────────────────────────────

pub(super) fn id_to_lud(id: u32) -> LightUserData {
    LightUserData(id as usize as *mut c_void)
}

/// Read the id out of a wrapper table's `T[0]` lightuserdata (`0x701bd0` writes it).
pub(crate) fn decode_id(this: &Table) -> mlua::Result<u32> {
    match this.raw_get::<Value>(0)? {
        Value::LightUserData(l) => Ok(l.0 as usize as u32),
        _ => Err(mlua::Error::runtime(
            "not a benilla frame/region object (missing T[0] identity)",
        )),
    }
}

/// Resolve `self` (a frame wrapper) to its live [`FrameHandle`].
pub(super) fn frame_handle_of(lua: &Lua, this: &Table) -> mlua::Result<FrameHandle> {
    let id = decode_id(this)?;
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    model
        .id_to_frame
        .get(&id)
        .copied()
        .ok_or_else(|| mlua::Error::runtime("stale or invalid frame handle"))
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Wrapper cache (`0x701bd0`)
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// The wrapper table for a frame id, created and cached on first use and published to `_G` under
/// the frame's name if that global is free. Call it with no model borrow alive.
pub(super) fn frame_wrapper(lua: &Lua, id: u32) -> mlua::Result<Table> {
    let wrappers: Table = lua.named_registry_value(REG_WRAPPERS)?;
    if let Value::Table(t) = wrappers.get::<Value>(id)? {
        return Ok(t);
    }
    let t = lua.create_table()?;
    t.raw_set(0, Value::LightUserData(id_to_lud(id)))?;
    // The kind picks the metatable once, here: a frame's kind never changes and `id_to_frame`
    // never loses an entry, so the choice cannot go stale.
    let (kind, name): (Option<FrameKind>, Option<String>) = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        let frame = model
            .id_to_frame
            .get(&id)
            .and_then(|h| model.arena.frame(*h));
        (frame.map(|f| f.kind), frame.and_then(|f| f.name.clone()))
    };
    t.set_metatable(Some(frame_meta_for(lua, kind)?))?;
    wrappers.set(id, t.clone())?;
    if let Some(name) = name {
        publish_global(lua, &name, &t)?;
    }
    Ok(t)
}

/// Publish a wrapper to `_G[name]`, non-overwriting (`0x701bd0`).
pub(super) fn publish_global(lua: &Lua, name: &str, wrapper: &Table) -> mlua::Result<()> {
    let g = lua.globals();
    if g.get::<Value>(name)?.is_nil() {
        g.set(name, wrapper.clone())?;
    }
    Ok(())
}

/// A widget name argument already looked up in `_G` ([`prefetch_named_target`], then
/// [`resolve_named_target`]). The globals table is the client's only widget namespace (`0x701bd0`
/// publishes there; `0x76c760` and the anchor resolver `0x76c700` read back), so a global alias
/// of a frame (`Bar8Button1 = CharacterBag3Slot`) resolves like its name. The read is
/// `lua_gettable` (`0x6f3a40` → `0x6f7cf0`), which honours an `__index` on `_G` that may call back
/// into a widget binding, so it runs with no `Model` borrow alive and the decode comes after.
pub(crate) struct NamedTarget {
    /// The name as looked up, after any `$parent` expansion: the spelling a diagnostic quotes.
    pub(crate) name: String,
    /// `_G[name]` if it is a table (Lua type 5); its identity is decoded later.
    wrapper: Option<Table>,
}

impl NamedTarget {
    /// A name that is not UTF-8: it names nothing, and the miss path quotes `<non-utf8>`.
    pub(crate) fn unreadable() -> Self {
        Self {
            name: "<non-utf8>".into(),
            wrapper: None,
        }
    }
}

/// Read `_G[name]` for a name argument, with no `Model` borrow alive. `parent_base` is what a
/// leading `$parent` expands to, `None` where the binding does not expand: `0x76c5b0` is called
/// only from `SetName` (`0x76c691`) and the anchor resolver (`0x76c71c` in `0x76c700`), so
/// `SetPoint`/`SetAllPoints`' `relativeTo` expands, and `SetParent`, `SetScrollChild` and XML
/// `parent=`, which call `0x76c760` directly, do not.
pub(crate) fn prefetch_named_target(
    lua: &Lua,
    name: &str,
    parent_base: Option<&str>,
) -> NamedTarget {
    let name = match parent_base {
        Some(base) => crate::framexml::resolve_name(name, base),
        None => name.to_string(),
    };
    let wrapper = match lua.globals().get::<Value>(name.as_str()) {
        Ok(Value::Table(t)) => Some(t),
        _ => None,
    };
    NamedTarget { name, wrapper }
}

/// Decode a [`NamedTarget`] to a live frame's or region's id: `SetPoint` accepts any widget (the
/// `CScriptRegion` type id `[0xcf0c3c]`), while `SetParent` and `SetScrollChild` check the Frame
/// id `[0xcf0c10]` and narrow the result themselves.
pub(crate) fn resolve_named_target(model: &Model, target: &NamedTarget) -> Option<u32> {
    let id = decode_id(target.wrapper.as_ref()?).ok()?;
    (model.id_to_frame.contains_key(&id) || model.id_to_region.contains_key(&id)).then_some(id)
}

/// What a leading `$parent` expands to: the first non-empty name at or above `start` (`0x76c5b0`'s
/// `+0x9c` walk), else [`crate::framexml::DEFAULT_PARENT_NAME`] (`0x76c5dd`). `start` is the
/// anchoring widget's parent: a frame's enclosing frame, a region's owner.
pub(crate) fn parent_token_base(model: &Model, start: Option<FrameHandle>) -> String {
    let mut cur = start;
    while let Some(p) = cur {
        let Some(f) = model.arena.frame(p) else { break };
        if let Some(n) = f.name.as_deref().filter(|n| !n.is_empty()) {
            return n.to_string();
        }
        cur = f.parent;
    }
    crate::framexml::DEFAULT_PARENT_NAME.to_string()
}

/// [`parent_token_base`] for a frame's own anchors: the walk starts at its parent.
pub(crate) fn frame_parent_token_base(model: &Model, h: FrameHandle) -> String {
    parent_token_base(model, model.arena.frame(h).and_then(|f| f.parent))
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// String → enum parsing
// ─────────────────────────────────────────────────────────────────────────────────────────────

pub(super) fn point_from_str(s: &str) -> Option<Point> {
    Some(match s.to_ascii_uppercase().as_str() {
        "TOPLEFT" => Point::TopLeft,
        "TOP" => Point::Top,
        "TOPRIGHT" => Point::TopRight,
        "LEFT" => Point::Left,
        "CENTER" => Point::Center,
        "RIGHT" => Point::Right,
        "BOTTOMLEFT" => Point::BottomLeft,
        "BOTTOM" => Point::Bottom,
        "BOTTOMRIGHT" => Point::BottomRight,
        _ => return None,
    })
}

/// The inverse of [`point_from_str`]: the names of the client's point table `0x811a38`.
pub(super) fn point_name(p: Point) -> &'static str {
    match p {
        Point::TopLeft => "TOPLEFT",
        Point::Top => "TOP",
        Point::TopRight => "TOPRIGHT",
        Point::Left => "LEFT",
        Point::Center => "CENTER",
        Point::Right => "RIGHT",
        Point::BottomLeft => "BOTTOMLEFT",
        Point::Bottom => "BOTTOM",
        Point::BottomRight => "BOTTOMRIGHT",
    }
}

/// An enum token, trimmed and uppercased; trimmed here rather than at the attribute read, since a
/// `text=` value's spaces count. The reference's strata lookup compares the whole token,
/// case-insensitively (`0x6f17ee`), so a padded `"BACKGROUND "` misses there.
fn enum_token(s: &str) -> String {
    s.trim().to_ascii_uppercase()
}

/// A strata name. The reference's table (`0x8119f8`) holds the eight `BACKGROUND`..`TOOLTIP`;
/// `BLIZZARD` is a later client's, not 1.12's. Stratum 0, `WORLD`, has no name: only the
/// WorldFrame's constructor sets it (`0x481aff`).
pub(crate) fn strata_from_str(s: &str) -> Option<Strata> {
    Some(match enum_token(s).as_str() {
        "BACKGROUND" => Strata::Background,
        "LOW" => Strata::Low,
        "MEDIUM" => Strata::Medium,
        "HIGH" => Strata::High,
        "DIALOG" => Strata::Dialog,
        "FULLSCREEN" => Strata::Fullscreen,
        "FULLSCREEN_DIALOG" => Strata::FullscreenDialog,
        "TOOLTIP" => Strata::Tooltip,
        "BLIZZARD" => Strata::Blizzard,
        _ => return None,
    })
}

pub(super) fn draw_layer_from_str(s: &str) -> Option<DrawLayer> {
    Some(match enum_token(s).as_str() {
        "BACKGROUND" => DrawLayer::Background,
        "BORDER" => DrawLayer::Border,
        "ARTWORK" => DrawLayer::Artwork,
        "OVERLAY" => DrawLayer::Overlay,
        "HIGHLIGHT" => DrawLayer::Highlight,
        _ => return None,
    })
}

/// The inverse of [`draw_layer_from_str`]: the uppercase token `GetDrawLayer` answers.
pub(super) fn draw_layer_name(l: DrawLayer) -> &'static str {
    match l {
        DrawLayer::Background => "BACKGROUND",
        DrawLayer::Border => "BORDER",
        DrawLayer::Artwork => "ARTWORK",
        DrawLayer::Overlay => "OVERLAY",
        DrawLayer::Highlight => "HIGHLIGHT",
    }
}

/// A widget tag or `CreateFrame` type → its [`FrameKind`] ([`enum_token`]). Public so the app asks
/// this one mapping rather than keeping a copy.
pub fn frame_kind_from_tag(s: &str) -> Option<FrameKind> {
    frame_kind_from_str(s)
}

/// The type registry lookup `CreateFrame` and the XML loader share (`0x6ee280`, table
/// `[0xcee9d8]`): the tag's [`FrameKind`] if a factory is registered for it now. The WorldFrame's
/// record is released once the first one is made (`0x6ee439`), so a second one misses. On a miss
/// Lua raises and XML logs `Unknown frame type: %s` and skips the node.
pub(crate) fn registered_frame_kind(lua: &Lua, kind: &str) -> Option<FrameKind> {
    let frame_kind = frame_kind_from_str(kind)?;
    let one_shot_spent = frame_kind == FrameKind::WorldFrame
        && lua
            .app_data_ref::<Model>()
            .expect("model app_data")
            .world_frame_made;
    (!one_shot_spent).then_some(frame_kind)
}

fn frame_kind_from_str(s: &str) -> Option<FrameKind> {
    Some(match enum_token(s).as_str() {
        "FRAME" => FrameKind::Frame,
        // Registered at `0x495948`, the one row passing `1` as its third argument.
        "WORLDFRAME" => FrameKind::WorldFrame,
        // A plain `CSimpleFrame`: factory `0x495ba0` allocates `0x314` like `<Frame>`'s `0x6eec10`,
        // the ctor `0x506950` adds no field, and its paint `0x506a90` does nothing, so
        // `GetObjectType()` is "Frame" and `IsObjectType("TaxiRouteFrame")` false (`0x484800` is
        // not overridden). The flight lines are the FrameXML's own rotated textures.
        "TAXIROUTEFRAME" => FrameKind::Frame,
        "BUTTON" => FrameKind::Button,
        "CHECKBUTTON" => FrameKind::CheckButton,
        // Its own registered type (`0x4959a6`), not a Button: 1.12 has no Lua verb to take loot
        // slot N (`LootSlot` only continues a bind confirm), so a Button row could loot nothing.
        "LOOTBUTTON" => FrameKind::LootButton,
        "EDITBOX" => FrameKind::EditBox,
        "STATUSBAR" => FrameKind::StatusBar,
        "SLIDER" => FrameKind::Slider,
        "SCROLLFRAME" => FrameKind::ScrollFrame,
        "MODEL" => FrameKind::Model,
        "PLAYERMODEL" => FrameKind::PlayerModel,
        "DRESSUPMODEL" => FrameKind::DressUpModel,
        "TABARDMODEL" => FrameKind::TabardModel,
        "MESSAGEFRAME" => FrameKind::MessageFrame,
        "SCROLLINGMESSAGEFRAME" => FrameKind::ScrollingMessageFrame,
        "COLORSELECT" => FrameKind::ColorSelect,
        "SIMPLEHTML" => FrameKind::SimpleHtml,
        "MOVIEFRAME" => FrameKind::MovieFrame,
        "GAMETOOLTIP" => FrameKind::GameTooltip,
        "MINIMAP" => FrameKind::Minimap,
        _ => return None,
    })
}

/// A Lua number-ish → f32 (nil/other → 0.0), for offset/color args.
pub(super) fn as_f32(v: &Value) -> f32 {
    match v {
        Value::Number(n) => *n as f32,
        Value::Integer(i) => *i as f32,
        _ => 0.0,
    }
}

/// [`as_f32`] in `f64`, for `ColorSelect:SetColorRGB`, whose quantizer an `f32` detour could push
/// across a rounding boundary.
pub(super) fn as_f64(v: &Value) -> f64 {
    match v {
        Value::Number(n) => *n,
        Value::Integer(i) => *i as f64,
        _ => 0.0,
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// install
// ─────────────────────────────────────────────────────────────────────────────────────────────

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    // The wrapper cache and the per-frame script tables, as registry roots.
    lua.set_named_registry_value(REG_WRAPPERS, lua.create_table()?)?;
    lua.set_named_registry_value(REG_SCRIPTS, lua.create_table()?)?;

    // Opened before any method table is built; `region_map::install` below closes it.
    super::region_map::open_arms(lua);
    install_frame_methods(lua)?;
    super::region::install(lua)?;
    super::statusbar::install(lua)?;
    super::button::install(lua)?;
    super::editbox::install(lua)?;
    // After every table above exists: each of the 19 Region-map names gets one shared
    // implementation, so a method taken off a frame works on a texture.
    super::region_map::install(lua)?;

    // The per-kind metatable cache, holding the plain `Frame`'s, whose `__index` is the frame
    // method table.
    lua.set_named_registry_value(REG_KIND_METAS, lua.create_table()?)?;
    let frame_meta = frame_meta_for(lua, None)?;
    lua.set_named_registry_value(REG_FRAME_META, frame_meta.clone())?;
    // FrameScript init publishes it as `_G["__framescript_meta"]` (`0x7039f7`).
    lua.globals().set("__framescript_meta", frame_meta)?;

    let create_frame = lua.create_function(create_frame)?;
    lua.globals().set("CreateFrame", create_frame)?;

    // `GetScreenWidth`/`GetScreenHeight`: the screen root's size in UI units.
    lua.globals().set(
        "GetScreenWidth",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(model.screen.width())
        })?,
    )?;
    lua.globals().set(
        "GetScreenHeight",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(model.screen.height())
        })?,
    )?;

    // `SetupFullscreenScale(frame)` (`0x48c270`) sets only the frame's scale, through `SetScale`:
    // `min(0.75 · a, 1.0)` for the configured aspect `a`, the `gxResolution` width over height,
    // which is the window's here (the reference's `widescreen` CVar at 0 would make it 4:3).
    // `UIParent`'s scale does not enter.
    lua.globals().set(
        "SetupFullscreenScale",
        lua.create_function(|lua, frame: Value| {
            let Value::Table(frame) = frame else {
                return Err(mlua::Error::runtime("Usage: SetupFullscreenScale(frame)"));
            };
            if decode_id(&frame).is_err() {
                return Err(mlua::Error::runtime(
                    "SetupFullscreenScale(): Couldn't find 'this' in frame object",
                ));
            }
            if frame_handle_of(lua, &frame).is_err() {
                return Err(mlua::Error::runtime(
                    "SetupFullscreenScale(): Wrong object type, expected frame",
                ));
            }
            let scale = {
                let model = lua.app_data_ref::<Model>().expect("model");
                fullscreen_scale(model.screen.width() / model.screen.height())
            };
            frame.call_method::<()>("SetScale", scale)
        })?,
    )?;

    // `GetCursorPosition()`: the last cursor position the host fed, in screen UI units, y up, as
    // the reference returns it; a caller in a scaled frame divides by its `GetEffectiveScale()`.
    lua.globals().set(
        "GetCursorPosition",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(model.cursor_pos)
        })?,
    )?;

    // The modifier keys the app feeds each frame before any mouse event
    // ([`UiScript::set_modifiers`]), answered 1 or nil as in 1.12, never a boolean: an addon
    // testing `IsShiftKeyDown() == 1` (`ColorPickerPlus.lua:121`) reads `true` as not held.
    for (name, pick) in [
        ("IsShiftKeyDown", 0usize),
        ("IsControlKeyDown", 1),
        ("IsAltKeyDown", 2),
    ] {
        lua.globals().set(
            name,
            lua.create_function(move |lua, ()| {
                let model = lua.app_data_ref::<Model>().expect("model");
                let m = [model.modifiers.0, model.modifiers.1, model.modifiers.2];
                Ok(crate::script::binding_abi::flag(m[pick]))
            })?,
        )?;
    }

    Ok(())
}

/// Registry key of the per-kind frame metatables, each keyed by its kind's first method-table key,
/// `""` for a kind with none.
const REG_KIND_METAS: &str = "__benilla_frame_meta_by_kind";

/// The metatable a frame of `kind` wears, built once per kind and cached in [`REG_KIND_METAS`].
/// Its `__index` is a table, so a method lookup never leaves `luaV_gettable` for Rust. The kind's
/// method tables are linked, not merged, in the reference's probe order, each class's lookup
/// tail-calling its base's (`CheckButton`, `Button`, `Frame`): a later write to a table shows
/// through, and a plain Frame never reaches `StatusBar`'s methods, so `frame.SetValue` stays nil.
fn frame_meta_for(lua: &Lua, kind: Option<FrameKind>) -> mlua::Result<Table> {
    let chain = kind_method_registries(kind);
    let key = chain.first().copied().unwrap_or("");
    let metas: Table = lua.named_registry_value(REG_KIND_METAS)?;
    if let Value::Table(meta) = metas.raw_get::<Value>(key)? {
        return Ok(meta);
    }
    // Link `own -> base -> ... -> Frame`; relinking a base several kinds share is idempotent.
    let frame_methods: Table = lua.named_registry_value(REG_FRAME_METHODS)?;
    let mut below = frame_methods.clone();
    for reg in chain.iter().rev() {
        let own: Table = lua.named_registry_value(reg)?;
        let link = lua.create_table()?;
        link.set("__index", below)?;
        own.set_metatable(Some(link))?;
        below = own;
    }
    let meta = lua.create_table()?;
    meta.set("__index", below)?;
    metas.raw_set(key, meta.clone())?;
    Ok(meta)
}

/// The registry keys of a kind's own method tables in the client's lookup order (`CheckButton`'s
/// `0x79a5d0` tail-calls `Button`'s); empty for a plain kind or no live frame. Each chain must be
/// its own table followed by its base's whole chain, or [`frame_meta_for`] cannot link it.
fn kind_method_registries(kind: Option<FrameKind>) -> &'static [&'static str] {
    match kind {
        Some(FrameKind::StatusBar) => &[super::statusbar::REG_STATUSBAR_METHODS],
        Some(FrameKind::EditBox) => &[super::editbox::REG_EDITBOX_METHODS],
        Some(FrameKind::ScrollingMessageFrame) => {
            &[super::messageframe::REG_SCROLLINGMESSAGEFRAME_METHODS]
        }
        // A sibling of `ScrollingMessageFrame` with its own table, not a fallthrough to it.
        Some(FrameKind::MessageFrame) => &[super::messageframe::REG_MESSAGEFRAME_METHODS],
        Some(FrameKind::ScrollFrame) => &[super::scrollframe::REG_SCROLLFRAME_METHODS],
        Some(FrameKind::SimpleHtml) => &[super::simplehtml::REG_SIMPLEHTML_METHODS],
        Some(FrameKind::Slider) => &[super::slider::REG_SLIDER_METHODS],
        Some(FrameKind::ColorSelect) => &[super::colorselect::REG_COLORSELECT_METHODS],
        Some(FrameKind::Button) => &[super::button::REG_BUTTON_METHODS],
        // Its one method, then `Button`'s: `0x4c1be0` probes its map `0xb71b64`, then `0x782c90`.
        Some(FrameKind::LootButton) => &[
            super::loot::REG_LOOTBUTTON_METHODS,
            super::button::REG_BUTTON_METHODS,
        ],
        Some(FrameKind::CheckButton) => &[
            super::button::REG_CHECKBUTTON_METHODS,
            super::button::REG_BUTTON_METHODS,
        ],
        Some(FrameKind::Model) => &[super::modelframe::REG_MODEL_METHODS],
        // Its 3 methods, then `Model`'s 23: `0x506260` misses into `0x76f870`.
        Some(FrameKind::PlayerModel) => &[
            super::modelframe::REG_PLAYERMODEL_METHODS,
            super::modelframe::REG_MODEL_METHODS,
        ],
        // 3 of its own (`0x84f190`), then `PlayerModel`'s 3 (`0x506260`), then `Model`'s 23
        // (`0x76f870`).
        Some(FrameKind::DressUpModel) => &[
            super::dressup::REG_DRESSUPMODEL_METHODS,
            super::modelframe::REG_PLAYERMODEL_METHODS,
            super::modelframe::REG_MODEL_METHODS,
        ],
        // 10 of its own (`0x84ee40`), then `PlayerModel`'s 3, then `Model`'s 23.
        Some(FrameKind::TabardModel) => &[
            super::tabard::REG_TABARDMODEL_METHODS,
            super::modelframe::REG_PLAYERMODEL_METHODS,
            super::modelframe::REG_MODEL_METHODS,
        ],
        Some(FrameKind::Minimap) => &[super::minimap::REG_MINIMAP_METHODS],
        Some(FrameKind::GameTooltip) => &[super::tooltip::REG_TOOLTIP_METHODS],
        _ => &[],
    }
}

/// `CreateFrame(kind, name?, parent?, inherits?)` (`0x7060b0`), the runtime frame factory.
/// `inherits` names one template, applied after the frame is made
/// ([`crate::loader::apply_template`]) so its `OnLoad` sees the frame's real name and its `$parent`
/// resolves against the caller's name. An unknown template raises `Couldn't find inherited node`
/// and creates nothing (`0x7061dd`); a known but unusable one (not virtual, or the wrong shape)
/// only warns, since the lookup itself succeeded.
pub(super) fn create_frame(
    lua: &Lua,
    (kind, name, parent, inherits): (String, Option<Value>, Option<Value>, Option<Value>),
) -> mlua::Result<Table> {
    // An unknown type raises. The reference's message (`0x872fa8`) capitalises `Unknown`, and it
    // tests the template first (`0x7061dd`), raising for the type only at `0x7062a0`.
    let frame_kind = registered_frame_kind(lua, &kind)
        .ok_or_else(|| mlua::Error::runtime(format!("CreateFrame: unknown frame type '{kind}'")))?;
    // `name` and `inherits` go through `lua_tostring` (`0x6f3690`) unguarded, so a number is a
    // string: `CreateFrame("Frame", 5)` names the frame "5". For `inherits` the conversion runs
    // before the string-type gate and retags the slot (`0x70613f`, `0x6f7cb1`), so a number there
    // raises as a missing template, where `CreateTexture` ignores one.
    let name: Option<String> = name.as_ref().and_then(|v| optional_string(lua, v));
    let template: Option<String> = inherits.as_ref().and_then(|v| optional_string(lua, v));

    // A table is the parent and a name string is looked up; anything else is no parent. The
    // reference raises `Usage: CreateFrame(...)` (`0x872f60`) for anything but a table or nil
    // (`0x706112`).
    let parent_handle: Option<FrameHandle> = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        match &parent {
            Some(Value::Table(t)) => decode_id(t)
                .ok()
                .and_then(|id| model.id_to_frame.get(&id).copied()),
            Some(Value::String(s)) => s.to_str().ok().and_then(|n| model.arena.lookup(n.as_ref())),
            _ => None,
        }
    };

    // The reference silently ignores a fourth argument that is neither string nor number
    // (`0x7061cb`); the warning is ours and changes nothing.
    if template.is_none() {
        if let Some(v) = inherits.as_ref().filter(|v| !v.is_nil()) {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.record_warning(format!(
                "CreateFrame: the 4th argument (inherits) must be a template-name string, got {}; \
                 ignored for '{}'",
                v.type_name(),
                name.as_deref().unwrap_or("<unnamed>")
            ));
        }
    }

    // The template lookup comes before anything is built: a miss (`0x7061dd` → `0x6ee6f0`) raises
    // at `0x7061ed`, before the node build (`0x706208`) and the name store (`0x70622d`), so it
    // leaves no partial widget and no global. The raise never returns and `pcall` catches it, as
    // the widget dispatcher (`0x704f10`) does for every handler.
    if let Some(t) = template.as_deref().filter(|t| !t.is_empty()) {
        let known = {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let templates = model.framexml_templates.borrow();
            // One name, verbatim, case-folded, as `framexml::expand` resolves it.
            templates.contains_key(t) || templates.keys().any(|k| k.eq_ignore_ascii_case(t))
        };
        if !known {
            return Err(mlua::Error::runtime(format!(
                "CreateFrame(): Couldn't find inherited node \"{t}\""
            )));
        }
    }

    // A leading `$parent` in the name expands as in XML: `CreateFrame` sets `name=` on a synthetic
    // node (`0x70622d`) for the XML builder `0x6ee280`, so the name reaches `SetName` (`0x76c650`),
    // whose `0x76c691` call expands it against the parent's first named ancestor.
    let name = name.map(|n| {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        crate::framexml::resolve_name(&n, &parent_token_base(&model, parent_handle))
    });

    // Create in the arena, mint the id, seed a default layout input. All under one write borrow.
    let id = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let h = model.arena.create(frame_kind, name, parent_handle);
        if frame_kind == FrameKind::WorldFrame {
            model.world_frame_made = true;
        }
        // A child enters its parent's stratum at the parent's level + 1; an explicit
        // `frameStrata` or `frameLevel` still overrides it afterwards.
        if let Some(p) = parent_handle {
            if let Some((pstrata, plevel)) = model.arena.frame(p).map(|f| (f.strata, f.level)) {
                model.arena.set_frame_strata(h, pstrata);
                model.arena.set_frame_level(h, plevel + 1, false);
            }
        }
        let id = model.frame_id(h);
        model.layout_inputs.entry(h).or_default();
        id
    };

    // The wrapper exists, published under its name, before the template's `OnLoad` runs inside
    // this call, so the handler can reach the frame by name.
    let wrapper = frame_wrapper(lua, id)?;
    if let Some(template) = template {
        let messages = crate::loader::apply_template(lua, &wrapper, &kind, &template);
        if !messages.is_empty() {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            for m in messages {
                model.record_warning(m);
            }
        }
    }
    Ok(wrapper)
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Frame method surface
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// Build the shared frame method table from its five clusters and publish it to the registry.
fn install_frame_methods(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;
    frame_state::install(lua, &m)?;
    layout_methods::install(lua, &m)?;
    events_regions::install(lua, &m)?;
    movable::install(lua, &m)?;
    toplevel::install(lua, &m)?;
    lua.set_named_registry_value(REG_FRAME_METHODS, m)?;
    Ok(())
}

/// `0x48c270`'s scale: `0.75 · aspect` below 1.0, else 1.0; NaN takes the 1.0 leg, as the
/// reference's `fcomp` fails every ordered test.
pub(crate) fn fullscreen_scale(aspect: f32) -> f32 {
    let g = 0.75 * aspect;
    if g < 1.0 {
        g
    } else {
        1.0
    }
}

#[cfg(test)]
mod fullscreen_scale_tests {
    use super::fullscreen_scale;
    use crate::script::UiScript;

    #[test]
    fn the_scale_is_three_quarters_of_the_aspect_capped_at_one() {
        assert_eq!(fullscreen_scale(4.0 / 3.0), 1.0);
        assert_eq!(fullscreen_scale(16.0 / 9.0), 1.0);
        assert!((fullscreen_scale(5.0 / 4.0) - 0.9375).abs() < 1e-6);
        assert_eq!(fullscreen_scale(f32::NAN), 1.0);
    }

    #[test]
    fn the_verb_scales_the_frame_and_raises_its_three_strings() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(1280.0, 1024.0);
        s.run(r#"f = CreateFrame("Frame", "FS") SetupFullscreenScale(f)"#)
            .unwrap();
        assert!((s.eval::<f64>("return f:GetScale()").unwrap() - 0.9375).abs() < 1e-6);
        s.set_screen_size(1600.0, 900.0);
        s.run("SetupFullscreenScale(f)").unwrap();
        assert_eq!(s.eval::<f64>("return f:GetScale()").unwrap(), 1.0);
        for (call, needle) in [
            (
                "SetupFullscreenScale()",
                "Usage: SetupFullscreenScale(frame)",
            ),
            (
                "SetupFullscreenScale(7)",
                "Usage: SetupFullscreenScale(frame)",
            ),
            (
                "SetupFullscreenScale({})",
                "Couldn't find 'this' in frame object",
            ),
            (
                "SetupFullscreenScale(f:CreateTexture())",
                "Wrong object type, expected frame",
            ),
        ] {
            let err = s.run(call).unwrap_err().to_string();
            assert!(err.contains(needle), "{call}: {err}");
        }
    }
}
