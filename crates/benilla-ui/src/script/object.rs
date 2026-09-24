//! The FrameScript object model, frame side — `CreateFrame`, the frame metatable +
//! wrapper cache, and the shared frame method surface (decision 0068 v1). The region side lives in
//! [`super::region`]; per-kind frame methods in their own modules ([`super::statusbar`], …),
//! dispatched by [`kind_method_registries`] before the shared table.
//!
//! A frame's Lua value is a table `T` with `T[0] = lightuserdata(id)` (the id encoding stands in for
//! the client's `CScriptObject*`; `0x701bd0` writes `T[0] = lightuserdata(this)` — see the module
//! docs on ids) and one **shared** metatable whose `__index` is a Rust function that dispatches a
//! method name to its implementation. Wrappers are cached in a Lua-side registry table keyed by id,
//! so `GetParent()` (etc.) returns the *same* table every time (stable identity). Named frames
//! auto-publish to `_G`, non-overwriting (`0x701bd0`). Region
//! leaves (`Texture`/`FontString`) use the same `T[0]` scheme but a *distinct* metatable (the "tag"
//! that separates their method surface from frames').
//!
//! The whole surface honors the MAXCSTACK discipline: the two metatables, the two method tables, and
//! the wrapper cache live in the Lua registry (named keys); Rust holds none of them across calls.

use std::ffi::c_void;

use mlua::{LightUserData, Lua, ObjectLike, Table, Value};

use super::binding_abi::optional_string;
use super::{Model, REG_FRAME_META, REG_FRAME_METHODS, REG_SCRIPTS, REG_WRAPPERS};
use crate::layout::Point;
use crate::order::{DrawLayer, Strata};
use crate::widget::{FrameHandle, FrameKind};

// The frame method-table clusters — split out purely for size (each module's own doc says what
// lives there); this file keeps the shared id/handle plumbing, `install`, and `CreateFrame`.
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
// Wrapper cache (`0x701bd0`: lazy bind + named _G publish, non-overwriting)
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// Get-or-create the wrapper table for a frame id, caching it and (if the frame is named and `_G`
/// has no such global yet) publishing it to `_G`. Callers must hold **no** model borrow (this takes
/// a short one to read the frame's name).
pub(super) fn frame_wrapper(lua: &Lua, id: u32) -> mlua::Result<Table> {
    let wrappers: Table = lua.named_registry_value(REG_WRAPPERS)?;
    if let Value::Table(t) = wrappers.get::<Value>(id)? {
        return Ok(t);
    }
    let t = lua.create_table()?;
    t.raw_set(0, Value::LightUserData(id_to_lud(id)))?;
    // The frame's kind and name in one borrow. **The KIND picks the metatable, here rather than
    // per lookup** — the region side's shape since its leaves split, and decision 2310's whole
    // change on this side: a wrapper is built once per frame, so resolving the class chain at
    // construction costs nothing, while resolving it inside `__index` put a Rust call, a model
    // borrow and a registry lookup in front of EVERY widget method access in the client. A frame's
    // kind never changes and `id_to_frame` never loses an entry, so the choice cannot go stale.
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

/// A widget **name string** argument, already looked up in `_G` — every Lua binding that takes
/// "a frame or its name" resolves one of these ([`prefetch_named_target`] then
/// [`resolve_named_target`]).
///
/// **The client has ONE widget namespace and it is the Lua globals table.** The wrapper binder
/// `0x701bd0` publishes `_G[name] = T` for a named widget (non-overwriting, via `lua_settable`),
/// and every by-name resolver reads that table back — `0x76c760` (globals index → Lua type 5 →
/// **raw** `t[0]` → `lua_touserdata` → `IsA`; `SetParent`'s string arm `0x7a1550`, `SetScrollChild`
/// `0x790fa0`, the XML `parent=` chain) and the layout-vtable `+0x28` entry `0x76c700`, which
/// expands a leading `$parent` (`0x76c5b0`) and then calls `0x76c760`. There is no engine-side
/// name→frame map behind either: for frames and regions a name's only effects are the copy into
/// `[widget+0x98]` and that `_G` publish. (`CSimpleFont` names are the one exception — they key a
/// Storm hash at `0xcf4e78` — and fonts are not anchor targets.) Full addresses:
/// `0x76c5b0`/`0x76c700`/`0x76c760`/`0x7a2540`.
///
/// So the name is not a *frame name* — it is a **global**, and a frame's published name is only the
/// commonest way one comes to exist. Resolving against a private registry instead makes an alias
/// invisible: `Bar8Button1 = CharacterBag3Slot` at an addon's file scope is a global that is no
/// frame's name, and Bartender2 anchors every bar's buttons through exactly those (decision 2105).
///
/// ## Why the lookup is its own phase
///
/// The reference's read is **`lua_gettable 0x6f3a40` → `luaV_index 0x6f7cf0`**, not `lua_rawget`
/// (the binary holds both, 0xc0 apart; only the former walks `__index`, MAXTAGLOOP 100). So a
/// metatable on `_G` is honoured — and a `__index` can be a *Lua* function that calls straight back
/// into a widget binding. Running that under a live `Model` guard would take a second borrow and
/// panic, so the `_G` read is done with **no guard alive** and the decode happens after.
pub(crate) struct NamedTarget {
    /// The name as the client looked it up — after `$parent` expansion where the caller allows it.
    /// This is the spelling a diagnostic must quote.
    pub(crate) name: String,
    /// `_G[name]`, kept only when it is a table (the Lua-type-5 gate). Identity is checked later.
    wrapper: Option<Table>,
}

impl NamedTarget {
    /// A name argument we could not even read (a non-UTF-8 Lua string) — it names nothing, and the
    /// caller's miss path quotes this placeholder the way it always has.
    pub(crate) fn unreadable() -> Self {
        Self {
            name: "<non-utf8>".into(),
            wrapper: None,
        }
    }
}

/// Read `_G[name]` for a name-string argument. **Call with no `Model` borrow alive** — see
/// [`NamedTarget`].
///
/// `parent_base` is the name a leading `$parent` expands to, or `None` for the bindings that do not
/// expand it. `0x76c5b0` has exactly two call sites — `SetName 0x76c691` and `0x76c71c` inside the
/// layout resolver `0x76c700` — so `SetPoint`/`SetAllPoints`' `relativeTo` **is** expanded at
/// runtime, and `SetParent`, `SetScrollChild` and the XML `parent=` chain (which call `0x76c760`
/// directly) are **not**.
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

/// Decode a [`NamedTarget`] to the stable id of a live frame **or** region (the namespace is one —
/// a Texture or FontString publishes into `_G` on the same rule a Frame does).
///
/// `SetPoint` type-checks against the **root** `CScriptRegion` id `[0xcf0c3c]`, which every widget
/// accepts; `SetParent`/`SetScrollChild` use the narrower Frame id `[0xcf0c10]`, so those callers
/// narrow the result to `id_to_frame` themselves.
pub(crate) fn resolve_named_target(model: &Model, target: &NamedTarget) -> Option<u32> {
    let id = decode_id(target.wrapper.as_ref()?).ok()?;
    (model.id_to_frame.contains_key(&id) || model.id_to_region.contains_key(&id)).then_some(id)
}

/// The name a leading `$parent` expands to: the first widget at or above `start` with a non-empty
/// name (`0x76c5b0`'s `+0x9c` walk, skipping an empty `GetName`), and
/// [`crate::framexml::DEFAULT_PARENT_NAME`] (`"Top"`) when there is none (`0x76c5dd`).
///
/// `start` is the anchoring widget's **parent**: a frame's own anchors say `$parent` of its
/// enclosing frame, and a region's say `$parent` of its owner — which is that region's `+0x9c`.
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

/// [`parent_token_base`] for a frame's own anchors — the walk starts at its parent.
pub(crate) fn frame_parent_token_base(model: &Model, h: FrameHandle) -> String {
    parent_token_base(model, model.arena.frame(h).and_then(|f| f.parent))
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// String → enum parsing (the client's tables, transcribed in order.rs / layout.rs)
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

/// The inverse of [`point_from_str`] (the client's point-id table `0x811a38`, names back out).
///
/// `pub(super)` because the REGION cluster reports the same names from the same table — one
/// spelling of the point set, not two (`region/layout.rs`'s `GetPoint`).
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

/// Every closed-vocabulary attribute parser **trims** before it matches (decision 1204).
///
/// Not tidiness: `zBar.xml:146` — a shipped, working 1.12 addon — declares
/// `frameStrata="BACKGROUND "` with a trailing space, and the real client took it. Ours refused,
/// the frame's whole `<Frames>` subtree went with it, and the addon never loaded. Whitespace is
/// never meaningful in an enum token, so trimming is both the lenient answer and the correct one;
/// it is applied at the three parsers rather than at the attribute read, because a `text=`
/// attribute's leading space IS meaningful and must not be touched.
fn enum_token(s: &str) -> String {
    s.trim().to_ascii_uppercase()
}

/// The reference's strata NAME table (`0x8119f8`) has eight rows, `BACKGROUND`..`TOOLTIP`; stratum
/// 0 (`WORLD`) has no name and no XML or Lua can put a frame there — the WorldFrame's constructor
/// is its only writer (decision 1984, `0x481aff`).
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

/// The inverse of [`draw_layer_from_str`] — the token `GetDrawLayer` answers, in the client's own
/// spelling (uppercase, the five `<Layer level=>` names).
pub(super) fn draw_layer_name(l: DrawLayer) -> &'static str {
    match l {
        DrawLayer::Background => "BACKGROUND",
        DrawLayer::Border => "BORDER",
        DrawLayer::Artwork => "ARTWORK",
        DrawLayer::Overlay => "OVERLAY",
        DrawLayer::Highlight => "HIGHLIGHT",
    }
}

/// A widget tag or `CreateFrame` type string → its [`FrameKind`], case- and separator-insensitive
/// ([`enum_token`]). Public because it is the ONE mapping: `benilla-app`'s `frame_flag_gate` sweep
/// needs it to ask the engine whether a tag is mouse-enabled by construction, and a second copy
/// there is exactly what drifted.
pub fn frame_kind_from_tag(s: &str) -> Option<FrameKind> {
    frame_kind_from_str(s)
}

/// **The type registry lookup both doors share** (`0x6ee280`'s table `[0xcee9d8]`, its only
/// reader) — the tag's [`FrameKind`] if a factory is registered under it right now, else `None`.
///
/// "Right now" is the WorldFrame's one-shot: the reference unlinks and releases that record the
/// moment the first `<WorldFrame>` is instantiated (`0x6ee439`), so a second one — from any XML, or
/// `CreateFrame("WorldFrame")` — takes the lookup's miss leg (decision 1984).
///
/// One function because the two doors disagree only in what a MISS does, never in what a miss is
/// (decision 2191): the Lua binding raises ([`create_frame`]), the XML loader logs
/// `"Unknown frame type: %s"` and skips the node (`crate::loader`) — both legs off the same
/// `0x6ee280`.
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
        // The world frame's own registered type (decision 1983; `0x495948` in the registration
        // batch, the one row passing `1` as its third argument).
        "WORLDFRAME" => FrameKind::WorldFrame,
        // `TaxiRouteFrame` — a registered `CreateFrame` type that is a `CSimpleFrame` and NOTHING
        // else, so it maps to `Frame` rather than earning a kind (decision 1828). Factory
        // `0x495ba0` allocates `0x314`, the same
        // size the plain-`<Frame>` factory `0x6eec10` allocates for the same base ctor `0x769090`,
        // and the ctor `0x506950` adds no field. Its vtables are the base's length exactly (36 + 11
        // slots), so it declares no new virtual; it overrides four slots, of which two are lifetime
        // plumbing and the other two — `EmitLayer 0x506a60` and a scale hook `0x506ac0` — are DEAD
        // in 5875: the paint callback `0x506a90` tail-calls `0x506aa0`, which is a bare `ret`, and
        // every module global it would fill is written by nothing.
        //
        // Mapping it to `Frame` is therefore the faithful answer, not a 1203 shortcut — and it is
        // faithful in the strong sense that `GetObjectType()` must return **"Frame"** and
        // `IsObjectType("TaxiRouteFrame")` must be **false**: `0x484800` is not overridden, so the
        // name lives only in the 22-entry tag→factory registry and never becomes a class identity.
        // The flight-path lines are plain `<Texture>` children the FrameXML creates and rotates
        // with the 8-argument `SetTexCoord` we already support; `TaxiRouteFrame.cpp` is a vestigial
        // C++ renderer that draws nothing.
        "TAXIROUTEFRAME" => FrameKind::Frame,
        "BUTTON" => FrameKind::Button,
        "CHECKBUTTON" => FrameKind::CheckButton,
        // A real registered type (`0x4959a6`), not an alias for Button — decision 1799. Registering
        // it AS a Button would load `LootFrame.xml` and leave every row dead, because 1.12 has no
        // Lua verb for "take slot N": `LootSlot` is the bind-confirm continuation, hard-wired to
        // `flag = 1`.
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

/// [`as_f32`]'s double-width sibling, for the shape-C positions whose store is `f64` — today only
/// `ColorSelect:SetColorRGB`, whose channels go through a quantizer where a detour via `f32` could
/// move a value across a rounding boundary (`colorselect`'s module doc).
pub(super) fn as_f64(v: &Value) -> f64 {
    match v {
        Value::Number(n) => *n,
        Value::Integer(i) => *i as f64,
        _ => 0.0,
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// install — build the method tables, metatables, CreateFrame, and registry roots
// ─────────────────────────────────────────────────────────────────────────────────────────────

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    // The wrapper cache + the per-frame script tables (Lua-side roots — the MAXCSTACK discipline).
    lua.set_named_registry_value(REG_WRAPPERS, lua.create_table()?)?;
    lua.set_named_registry_value(REG_SCRIPTS, lua.create_table()?)?;

    // The Region-map arm collection, opened before either method table is built and closed by
    // `region_map::install` below (its [`super::region_map::Arms`] doc says why it is app_data).
    super::region_map::open_arms(lua);
    install_frame_methods(lua)?;
    super::region::install(lua)?;
    super::statusbar::install(lua)?;
    super::button::install(lua)?;
    super::editbox::install(lua)?;
    // AFTER every table above exists: collapse the 19 Region-map names to one shared
    // implementation each, so a method pulled off a frame works on a texture (decision 1501).
    super::region_map::install(lua)?;

    // The per-kind metatable cache ([`frame_meta_for`]) and the base metatable in it: the one a
    // plain `Frame` wears, whose `__index` is the shared frame method table itself.
    lua.set_named_registry_value(REG_KIND_METAS, lua.create_table()?)?;
    let frame_meta = frame_meta_for(lua, None)?;
    lua.set_named_registry_value(REG_FRAME_META, frame_meta.clone())?;
    // FrameScript init (`0x7039f7`) publishes the shared metatable as _G["__framescript_meta"].
    lua.globals().set("__framescript_meta", frame_meta)?;

    // CreateFrame(kind, name?, parent?, inherits?) — a global (the loader's factory is `0x6ee280`;
    // this is the runtime one). `inherits` (4th) applies a registered template to the frame this
    // call just made, through `crate::loader::apply_template`.
    let create_frame = lua.create_function(create_frame)?;
    lua.globals().set("CreateFrame", create_frame)?;

    // GetScreenWidth/Height — the screen-root rect in UI units (the ref's tooltip side-pick reads
    // them: ContainerFrameItemButton_OnEnter anchors LEFT when the slot sits in the right half).
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

    // SetupFullscreenScale(frame) — the registered binding `0x48c270` the stock WorldMapFrame.xml,
    // CinematicFrame.xml and UIOptionsFrame.xml call from OnShow (decision 1980): the frame's scale
    // becomes `min(0.75 · a, 1.0)` for the CONFIGURED aspect `a` — the `gxResolution` width over
    // height (the `widescreen` CVar's default of 1 selects it; at 0 the aspect is 4:3 and the
    // scale 1) — and nothing else is written: no anchor, size or position, and `UIParent`'s
    // scale never enters. It is the same `CSimpleFrame::SetScale` the Lua method calls. The
    // window's aspect is the configured one here (the UI-unit rect keeps the pixel ratio); NaN
    // takes the 1.0 leg as the reference's `fcomp` does. The three raises are the binding's own.
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

    // GetCursorPosition() → x, y — the last cursor position the host fed (`mouse_move`/
    // `mouse_button`), UI units y-up like every other coordinate read — the SCREEN's units, which
    // is what the reference hands Lua too; a caller inside a scaled frame divides by its
    // `GetEffectiveScale()` (the reference's own `MouseIsOver` does, and the stock world map at
    // a scale under 1 is the case that made the division load-bearing here — decision 1985). The
    // world map polls this every OnUpdate for hover/click math.
    lua.globals().set(
        "GetCursorPosition",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(model.cursor_pos)
        })?,
    )?;

    // The modifier-key mirror ([`UiScript::set_modifiers`], fed by the app's input pass before
    // any mouse event each frame) — the reference handlers fork on these at click time
    // (ContainerFrame.lua's shift-split / ctrl-dressup, ActionBarFrame.xml's shift-pickup,
    // SpellBookFrame.lua's shift-pickup).
    //
    // **1/nil, not a Lua boolean.** These three shipped Era booleans on 0068's reasoning that
    // "every transcribed `if IsShiftKeyDown()` reads both identically" — true while we wrote every
    // caller, and false the day real 1.12 addons load (1188/1751). `ColorPickerPlus.lua:121` is
    // `if IsShiftKeyDown() == 1 then`, and a `true` reads there as "not held" (decision 2118).
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

/// The registry table of per-kind frame metatables, keyed by the head of the kind's method chain
/// (`""` for a kind with none). Lua-side, like every other root here — the MAXCSTACK discipline.
const REG_KIND_METAS: &str = "__benilla_frame_meta_by_kind";

/// The metatable a frame of `kind` wears — built once per kind, cached in [`REG_KIND_METAS`].
///
/// ## `__index` is a TABLE, and that is the whole point (decision 2310)
///
/// This used to be one shared metatable whose `__index` was a **Rust function** that resolved the
/// receiver's kind and walked its registries per lookup. Every `frame:SetPoint(...)` — every plain
/// `frame.Foo` read, including the `if frame.SetValue then` duck-type probe addons open with —
/// therefore crossed into Rust, took a model borrow, resolved a handle and did one to three
/// named-registry string lookups. Measured in a release build: **217 ns for a plain Frame, 320 ns
/// for a Button, against 8.7 ns for a plain Lua table `__index`.** A live sample of a quest-accept
/// spike with the director's addon set put ~20 % of the whole UI tick inside that metamethod
/// before any method body ran — Questie redraws its map notes with ~19 widget calls per cluster
/// over hundreds of clusters, and pays the tax on every one.
///
/// A table costs one `luaH_get` inside `luaV_gettable`, in C, with no call at all.
///
/// ## The chain is the reference's own, expressed as metatables
///
/// 1.12's widget classes are not a Lua metatable chain — each class owns a flat method map and its
/// lookup tail-calls exactly one base's lookup ([`super::region_map`]'s header has the tables). But
/// `luaV_gettable` follows a table `__index` iteratively, so a chain of metatables IS that probe
/// sequence: `CheckButton`'s map, miss, `Button`'s map, miss, `Frame`'s map. So rather than merging
/// each kind's methods into one flat copy, this links the existing tables — no copies to go stale
/// when a method table is written after the fact, and the per-kind surface stays exactly what
/// [`kind_method_registries`] declares. Duck typing is preserved by construction: a plain Frame's
/// chain never reaches `StatusBar`'s map, so `frame.SetValue` is still nil.
fn frame_meta_for(lua: &Lua, kind: Option<FrameKind>) -> mlua::Result<Table> {
    let chain = kind_method_registries(kind);
    let key = chain.first().copied().unwrap_or("");
    let metas: Table = lua.named_registry_value(REG_KIND_METAS)?;
    if let Value::Table(meta) = metas.raw_get::<Value>(key)? {
        return Ok(meta);
    }
    // Link `own -> base -> ... -> Frame`, idempotently: a base table shared by several kinds (every
    // button kind reaches `Button`'s) is simply re-pointed at the same place.
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

/// The registry keys of a frame's kind-specific method tables, **in resolution order** — the
/// client's own class chain (CheckButton's lookup `0x79a5d0` tail-calls Button's). Empty for a
/// plain kind, and for a wrapper whose frame is not live.
///
/// Every chain here is *its own table followed by its base's whole chain*, which is what lets
/// [`frame_meta_for`] realize it as a metatable chain instead of a search: `CheckButton`'s table
/// gets `Button`'s as its `__index`, `Button`'s gets `Frame`'s, and one `luaV_gettable` walks the
/// lot in C. Keep that property when adding a kind — a chain that is not a suffix of its base's
/// cannot be expressed this way.
fn kind_method_registries(kind: Option<FrameKind>) -> &'static [&'static str] {
    match kind {
        Some(FrameKind::StatusBar) => &[super::statusbar::REG_STATUSBAR_METHODS],
        Some(FrameKind::EditBox) => &[super::editbox::REG_EDITBOX_METHODS],
        Some(FrameKind::ScrollingMessageFrame) => {
            &[super::messageframe::REG_SCROLLINGMESSAGEFRAME_METHODS]
        }
        // Its SIBLING, with its own table — not a fallthrough to the scrolling one. Until this arm
        // existed a `<MessageFrame>` resolved no kind-specific method at all, which is what blocked
        // `EasyCopy`, `QuestHistory` and `QuestItem` on `UIErrorsFrame:AddMessage` (three of the
        // corpus's six method-class blockers) and made `SetInsertMode` unreachable.
        Some(FrameKind::MessageFrame) => &[super::messageframe::REG_MESSAGEFRAME_METHODS],
        Some(FrameKind::ScrollFrame) => &[super::scrollframe::REG_SCROLLFRAME_METHODS],
        Some(FrameKind::SimpleHtml) => &[super::simplehtml::REG_SIMPLEHTML_METHODS],
        Some(FrameKind::Slider) => &[super::slider::REG_SLIDER_METHODS],
        Some(FrameKind::ColorSelect) => &[super::colorselect::REG_COLORSELECT_METHODS],
        Some(FrameKind::Button) => &[super::button::REG_BUTTON_METHODS],
        // One method of its own, then all of Button's — the miss order of `0x4c1be0`, which probes
        // its own 1-entry map `0xb71b64` and tail-calls `Button`'s `0x782c90`.
        Some(FrameKind::LootButton) => &[
            super::loot::REG_LOOTBUTTON_METHODS,
            super::button::REG_BUTTON_METHODS,
        ],
        Some(FrameKind::CheckButton) => &[
            super::button::REG_CHECKBUTTON_METHODS,
            super::button::REG_BUTTON_METHODS,
        ],
        // The Model pane's own surface. Its absence is what stopped pfUI — and, through pfUI's
        // embedded copies, pfQuest, pfQuest-turtle and ShaguDPS — on `SetModel` after the whole
        // rest of their UI had already built (`super::modelframe`'s header).
        Some(FrameKind::Model) => &[super::modelframe::REG_MODEL_METHODS],
        // The chain, not a repetition: `CGCharacterModelBase`'s lookup `0x506260` probes its own
        // 3-entry map and on a miss tail-calls `CSimpleModel`'s `0x76f870`. Order is the miss
        // order — three names of its own, then all 23 of `Model`'s.
        Some(FrameKind::PlayerModel) => &[
            super::modelframe::REG_PLAYERMODEL_METHODS,
            super::modelframe::REG_MODEL_METHODS,
        ],
        // Three of its own, then PlayerModel's three, then Model's 23: `CGDressUpModelFrame`'s
        // lookup probes `0x84f190` and misses into `CGCharacterModelBase`'s `0x506260`, which
        // misses into `CSimpleModel`'s `0x76f870` (1969).
        Some(FrameKind::DressUpModel) => &[
            super::dressup::REG_DRESSUPMODEL_METHODS,
            super::modelframe::REG_PLAYERMODEL_METHODS,
            super::modelframe::REG_MODEL_METHODS,
        ],
        // Ten of its own (`0x84ee40`), then PlayerModel's three, then Model's 23 — the same
        // derived → base probe as its sibling (1977).
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

/// `CreateFrame(kind, name?, parent?, inherits?)` — the runtime frame factory.
///
/// The fourth argument is a **template name list**, and honouring it is what makes
/// `CreateFrame("Button", "MyButton", UIParent, "UIPanelButtonTemplate")` — the corpus's most
/// common single line — produce a button with art, regions, scripts and a fired `OnLoad` rather
/// than an empty frame. The frame is created first and decorated second
/// ([`crate::loader::apply_template`]), in that order and never the other way: the template's own
/// `OnLoad` must be able to see the frame's real name, and `$parent` inside the template must
/// resolve against **the caller's** name, not the template's.
///
/// An **unresolvable** template name raises `CreateFrame(): Couldn't find inherited node "%s"` and
/// creates nothing — the reference's own behaviour, byte-verified (`0x7061dd` → `luaL_error`, which
/// never returns). A template that resolves but is *unusable in some other way* — declared without
/// `virtual="true"`, or of a shape that does not fit the kind asked for — is still a warning plus a
/// working frame, because the registry lookup itself succeeded and that is the only thing the miss
/// branch tests.
pub(super) fn create_frame(
    lua: &Lua,
    (kind, name, parent, inherits): (String, Option<Value>, Option<Value>, Option<Value>),
) -> mlua::Result<Table> {
    // The Lua door's miss RAISES: `0x7060b0` reaches `"CreateFrame: Unknown frame type '%s'"`
    // (`0x872fa8`) through `luaL_error 0x6f4940`, which never returns. The XML door's does not —
    // see [`registered_frame_kind`].
    let frame_kind = registered_frame_kind(lua, &kind)
        .ok_or_else(|| mlua::Error::runtime(format!("CreateFrame: unknown frame type '{kind}'")))?;
    // **`name` and `inherits` are `lua_tostring` positions, and a NUMBER is a string to it.**
    // `0x7060b0` reads both through `0x6f3690` with no type guard at all, so `CreateFrame("Frame",
    // 5)` names the frame `"5"` — a `Value::String`-only match drops it.
    //
    // The fourth argument's quirk, which is real and reads like a bug in the client: `lua_tostring`
    // runs at `0x70613f` **before** the `cmp 4` gate, and `0x6f7cb1 mov dword ptr [esi],0x4`
    // retags the number's stack slot **in place** — so by the time the tag is tested it IS a
    // string. `CreateFrame("Frame", nil, nil, 5)` therefore raises `Couldn't find inherited node
    // "5"`, while `f:CreateTexture(nil, nil, 5)` silently ignores the 5, because the region
    // constructors ask `lua_type` first and never coerce. Same value, opposite outcome, decided by
    // argument order alone — which is why this is spelled out rather than shared with them.
    let name: Option<String> = name.as_ref().and_then(|v| optional_string(lua, v));
    let template: Option<String> = inherits.as_ref().and_then(|v| optional_string(lua, v));

    // Resolve the parent handle (a wrapper table, or a name string) under a short read borrow.
    //
    // **The name-string arm is OURS, and the reference RAISES on it** — `0x7060b0` asks
    // `lua_type` three times and `cmp eax,5`: a table is the parent, nil/absent means no parent,
    // and a string takes the error leg.
    //
    // It is **parked, not pruned**, on the same footing as `TEXTURE_ONLY_METHODS`' tail three: it
    // is known to raise, but the exact message was never quoted, and inventing the text of a
    // client error is exactly what decision 1721 was written about — a plausible string in the
    // voice of a finding. Measured meanwhile: **zero** call sites pass a string parent, in the
    // 109-folder corpus or in our own FrameXML, so the superset is unreachable in practice and
    // removing it blind would buy nothing. Pruning it needs the message, not more confidence.
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

    // A 4th argument that is present but neither a string nor a number is **silently ignored by
    // the reference** — `lua_tostring` returns NULL for it and the `cmp 4` gate then falls into
    // the ordinary construction path. So this is a benilla-only diagnostic and stays a WARNING,
    // never a raise: it tells a reader something was dropped without changing what runs.
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

    // ── THE TEMPLATE LOOKUP HAPPENS HERE — before anything is constructed or published ──────────
    //
    // `0x7061dd` calls the registry lookup `0x6ee6f0` and, on NULL, falls into
    // `luaL_error(L, "CreateFrame(): Couldn't find inherited node \"%s\"")` at `0x7061ed`. That
    // never returns — `luaG_errormsg`/`luaD_throw` contain no `ret` at all and end in `longjmp`, so
    // the `xor eax,eax; ret` after the call is dead code. The frame is **not created**, no global
    // is published, and the calling statement is abandoned; the error is ordinary Lua propagation
    // and `pcall`-catchable, which is how the widget dispatcher (`0x704f10`, every handler under
    // `lua_pcall`) turns it into one dead handler rather than a dead client.
    //
    // We used to warn and hand back a usable bare frame, and the doc above said the reference
    // "creates the frame anyway". That claim is refuted at the bytes. It is the same class as 1249:
    // a failure the client reports and we swallowed — and swallowing is the worse half, because the
    // addon then runs on a frame with none of the art, regions or scripts it asked for and fails
    // later, somewhere unrelated.
    //
    // Ordering is the pinned part, not a detail: the miss branch precedes the synthetic-node build
    // (`0x706208`) and the `name` store (`0x70622d`), so a miss leaves no partial widget and no
    // orphan global behind. Doing this after construction would leave both.
    if let Some(t) = template.as_deref().filter(|t| !t.is_empty()) {
        let known = {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let templates = model.framexml_templates.borrow();
            // One name, verbatim, folded — the law `framexml::expand` resolves under.
            templates.contains_key(t) || templates.keys().any(|k| k.eq_ignore_ascii_case(t))
        };
        if !known {
            return Err(mlua::Error::runtime(format!(
                "CreateFrame(): Couldn't find inherited node \"{t}\""
            )));
        }
    }

    // **A leading `$parent` in the NAME is expanded, here as in XML.** `CreateFrame 0x7060b0`
    // does not store the name itself: it builds a synthetic node, sets `name=` on it
    // (`0x706225 push 0x838090 "name"` → `0x70622d` SetAttribute) alongside `parent=` and
    // `inherits=`, and hands it to the XML frame builder `0x6ee280` with the parent object in
    // `edx` — so the name reaches `CScriptRegion::SetName 0x76c650` by exactly the route an XML
    // `name=` does, and `0x76c691` is one of the expander's two call sites.
    //
    // The base is the frame's **parent's** first named ancestor — the same walk `$parent` in a
    // `relativeTo` takes, one link higher than the anchoring frame's own.
    //
    // Not academic: `FonzAppraiser` names every widget it builds this way (`"$parentDropdown"..n`,
    // `"$parentCloseButton"`, ~30 sites), and until decision 2176 made an unresolvable anchor
    // raise, the unexpanded name only showed up as a warning nobody read.
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
        // The client's CreateFrame inheritance (the ctor doc's "loader/CreateFrame concern" —
        // widget/mod.rs `create`): a child enters its PARENT's stratum at the parent's level + 1.
        // The ctor's bare MEDIUM/0 left a DIALOG-strata popup drawing its own translucent
        // backdrop OVER its buttons (the children stayed in MEDIUM — the abandon dialog's dark
        // buttons, pixel-diffed against the raw art). An explicit frameStrata/frameLevel attr
        // still overrides right after (apply_attrs).
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

    // The wrapper exists (and, if named, is published to `_G`) BEFORE the template runs — a
    // template's `OnLoad` fires inside this call, and the reference's addons rely on both facts:
    // `local b = CreateFrame(...)` hands back a frame whose OnLoad has already run, and that
    // handler can reach itself by name.
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
// Frame method surface — the table itself is built by the three clusters above
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// Build the shared frame method table from the five clusters ([`frame_state`], [`layout_methods`],
/// [`events_regions`], [`movable`], [`toplevel`]) and publish it to the registry.
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

/// `0x48c270`'s law: `g = 0.75 · aspect`, and the scale is `g` below 1.0, else 1.0 — NaN lands on
/// the 1.0 leg (the `fcomp` compare fails every ordered test).
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
