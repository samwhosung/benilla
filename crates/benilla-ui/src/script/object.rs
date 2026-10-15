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
use super::glue::VmKind;
use super::{Model, REG_FRAME_META, REG_FRAME_METHODS, REG_SCRIPTS, REG_WRAPPERS};
use crate::layout::Point;
use crate::order::{DrawLayer, Strata};
use crate::widget::{FrameHandle, FrameKind, KindState};

// The frame method clusters, split out for size.
pub(crate) mod anchor_args;
mod events_regions;
mod frame_state;
mod layout_methods;
pub(crate) use layout_methods::eff_scale;
pub(in crate::script) use layout_methods::{settle_frame, settle_region};
pub(crate) mod movable;
pub(crate) mod toplevel;
pub(crate) use movable::{cancel_on_hide, pump_move_or_size, FrameMove};

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

/// A strata name, from the reference's eight-row table (`0x8119f8`), `BACKGROUND`..`TOOLTIP`, walked
/// by `0x6f17d0`. Stratum 0, `WORLD`, has no name: only the WorldFrame's constructor sets it
/// (`0x481aff`).
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

/// A widget tag or `CreateFrame` type → its [`FrameKind`] ([`enum_token`]), whichever registration
/// makes it known. Public so the app asks this one mapping rather than keeping a copy.
pub fn frame_kind_from_tag(s: &str) -> Option<FrameKind> {
    frame_type(s).map(|t| t.kind)
}

/// The type registry lookup `CreateFrame` and the XML loader share (`0x6ee280`, table
/// `[0xcee9d8]`): the tag's [`FrameKind`] if a factory is registered for it now. The WorldFrame's
/// record is released once the first one is made (`0x6ee439`), so a second one misses. On a miss
/// Lua raises and XML logs `Unknown frame type: %s` and skips the node.
pub(crate) fn registered_frame_kind(lua: &Lua, kind: &str) -> Option<FrameKind> {
    let t = frame_type(kind)?;
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    let registered = match t.factory {
        Factory::Startup => true,
        Factory::InGame => model.vm == VmKind::InGame,
    };
    let one_shot_spent = t.kind == FrameKind::WorldFrame && model.world_frame_made;
    (registered && !one_shot_spent).then_some(t.kind)
}

/// `ModelFFX`, the glue's `CSimpleModelFFX` (factory `0x46fa00`): a `CSimpleModel` (its ctor
/// `0x46fa30` runs `0x76c8e0`, then swaps the vtable) answering as `Model` (`0x470030`) with
/// `Model`'s methods and `LoadXML` (`0x76cac0`), which differs in its OnShow (`0x46fa60`) and its
/// paint (`0x46fa90`, [`crate::widget::ModelState::ffx`]).
pub(crate) fn is_model_ffx(kind: &str) -> bool {
    enum_token(kind) == "MODELFFX"
}

/// When a frame type's factory is registered (`0x6ee880`) into the process-wide registry, which
/// nothing clears before shutdown (`0x6ee800`, from `0x4036fc`).
#[derive(Clone, Copy)]
enum Factory {
    /// Before any FrameScript state is built: the 13 base types (`0x6ef070`, from `0x402af3`), and
    /// `ModelFFX`, which the glue manager's init registers (`0x46a4fe`) before its first build
    /// (`0x46a505`), so the in-game state has it too.
    Startup,
    /// `UI_Init`'s eight (`0x495940`, from `0x48feb5`). They stay registered after a logout, so
    /// only a glue state that never entered the world lacks them, which is the one benilla builds.
    InGame,
}

/// A registered type: the [`FrameKind`] its factory builds, and its registration.
struct FrameType {
    kind: FrameKind,
    factory: Factory,
}

fn frame_type(s: &str) -> Option<FrameType> {
    use Factory::{InGame, Startup};
    let (kind, factory) = match enum_token(s).as_str() {
        "FRAME" => (FrameKind::Frame, Startup),
        // Registered at `0x495948`, the one row passing `1` as its third argument.
        "WORLDFRAME" => (FrameKind::WorldFrame, InGame),
        // A plain `CSimpleFrame`: factory `0x495ba0` allocates `0x314` like `<Frame>`'s `0x6eec10`,
        // the ctor `0x506950` adds no field, and its paint `0x506a90` does nothing, so
        // `GetObjectType()` is "Frame" and `IsObjectType("TaxiRouteFrame")` false (`0x484800` is
        // not overridden). The flight lines are the FrameXML's own rotated textures.
        "TAXIROUTEFRAME" => (FrameKind::Frame, InGame),
        "BUTTON" => (FrameKind::Button, Startup),
        "CHECKBUTTON" => (FrameKind::CheckButton, Startup),
        // Its own registered type (`0x4959a6`), not a Button: 1.12 has no Lua verb to take loot
        // slot N (`LootSlot` only continues a bind confirm), so a Button row could loot nothing.
        "LOOTBUTTON" => (FrameKind::LootButton, InGame),
        "EDITBOX" => (FrameKind::EditBox, Startup),
        "STATUSBAR" => (FrameKind::StatusBar, Startup),
        "SLIDER" => (FrameKind::Slider, Startup),
        "SCROLLFRAME" => (FrameKind::ScrollFrame, Startup),
        "MODEL" => (FrameKind::Model, Startup),
        "MODELFFX" => (FrameKind::Model, Startup),
        "PLAYERMODEL" => (FrameKind::PlayerModel, InGame),
        "DRESSUPMODEL" => (FrameKind::DressUpModel, InGame),
        "TABARDMODEL" => (FrameKind::TabardModel, InGame),
        "MESSAGEFRAME" => (FrameKind::MessageFrame, Startup),
        "SCROLLINGMESSAGEFRAME" => (FrameKind::ScrollingMessageFrame, Startup),
        "COLORSELECT" => (FrameKind::ColorSelect, Startup),
        "SIMPLEHTML" => (FrameKind::SimpleHtml, Startup),
        "MOVIEFRAME" => (FrameKind::MovieFrame, Startup),
        "GAMETOOLTIP" => (FrameKind::GameTooltip, InGame),
        "MINIMAP" => (FrameKind::Minimap, InGame),
        _ => return None,
    };
    Some(FrameType { kind, factory })
}

/// A colour or coordinate argument by the reference's bare `lua_tonumber` (`0x6f3620`): a number,
/// or a string `luaO_str2d` (`0x6f5900`) reads as one, and 0.0 for anything else, with no type gate
/// and no raise. [`super::binding_abi::coerced_number`] narrowed to the `f32` the widgets store.
pub(super) fn tonumber_f32(lua: &Lua, v: &Value) -> f32 {
    super::binding_abi::coerced_number(lua, Some(v.clone())) as f32
}

/// A colour's alpha argument: `lua_isnumber`-gated (`0x781037`, `0x79f5a2`, ...), so a numeric
/// string counts and anything else, a table or boolean included, stages 1.0 without raising.
pub(super) fn alpha_f32(lua: &Lua, v: &Value) -> f32 {
    lua.coerce_number(v.clone())
        .ok()
        .flatten()
        .map_or(1.0, |n| n as f32)
}

/// [`tonumber_f32`] in `f64`, for `ColorSelect:SetColorRGB`, whose quantizer an `f32` detour could
/// push across a rounding boundary.
pub(super) fn tonumber_f64(lua: &Lua, v: &Value) -> f64 {
    super::binding_abi::coerced_number(lua, Some(v.clone()))
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
    Ok(())
}

/// The frame globals only the in-game state registers (table `0x83de68`, `UI_Init`'s): the glue
/// state's `GetScreenWidth`/`GetScreenHeight` are other bindings (`0x46dd30`/`0x46dd80`), and it
/// has no modifier-key or fullscreen-scale verb.
pub(super) fn install_in_game(lua: &Lua) -> mlua::Result<()> {
    // `GetScreenWidth`/`GetScreenHeight` (`0x48b480`/`0x48b4d0`): the screen root's size divided by
    // `UIParent`'s scale (`GetUiScale`, `0x494590`), so they answer in `UIParent`'s units.
    lua.globals().set(
        "GetScreenWidth",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(model.screen.width() / super::ui_scale::ui_parent_scale(&model))
        })?,
    )?;
    lua.globals().set(
        "GetScreenHeight",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(model.screen.height() / super::ui_scale::ui_parent_scale(&model))
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
        Some(FrameKind::MovieFrame) => &[super::movieframe::REG_MOVIEFRAME_METHODS],
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
        // One name, verbatim, case-folded, as `framexml::expand` resolves it.
        let known = lua
            .app_data_ref::<Model>()
            .expect("model app_data")
            .framexml_templates
            .borrow()
            .contains(t);
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
        if is_model_ffx(&kind) {
            if let Some(KindState::Model(m)) = model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
                m.ffx = true;
            }
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
        // The builder's pre-load and post-load hooks (`0x6ee4d6`, `0x6ee4ea`): a frame born hidden
        // is deferred, as a hidden parent leaves it. A template's load opens its own window.
        model.layout_load_begin(h);
        // No handler is bound yet, so the resolve this owes fires nothing.
        let _ = model.layout_load_end(h);
        id
    };

    // The wrapper exists, published under its name, before the template's `OnLoad` runs inside
    // this call, so the handler can reach the frame by name.
    let wrapper = frame_wrapper(lua, id)?;
    if let Some(template) = template {
        let report = crate::loader::apply_template(lua, &wrapper, &kind, &template);
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        for (log, heard) in report.raised() {
            model.record_script_error(log.to_string(), heard.to_string());
        }
        for w in report.warnings {
            model.record_warning(w);
        }
    } else {
        super::tooltip_post_load(lua, &wrapper);
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
