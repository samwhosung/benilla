//! `SetPoint`/`SetAllPoints`' argument ladder — **one** transcription of `0x7a2540` / `0x7a2940`.
//!
//! The two bindings are `CScriptRegion` methods registered **once**, at `.data 0x87ca38` and
//! `0x87ca40` inside the 19-record table `[0x87c9b8, 0x87ca50)` whose registrar `0x7a2e70` supplies
//! base and count — so Frame, Texture, FontString and Model all run the *same* function
//! (`0x7a2540`/`0x7a2940`). This module is that single function's argument half; what each caller
//! keeps for itself is only the thing the reference reads off the receiver — the layout parent's
//! default id, and where the anchor is stored.
//!
//! **Every failure leg RAISES**, which is the point of the module (closing 2105's
//! "Named, not done"). `luaL_error` does not return: `luaG_errormsg [0x6fc780,0x6fc861)` and
//! `luaD_throw [0x6f5d80,0x6f5da5)` hold zero `ret`s, and `luaD_throw`'s *elided* epilogue is the
//! positive control that says so. There is no fallback and no silent no-op on any of them.
//!
//! The **XML** `<Anchor relativeTo=>` path is a different function and a different law — it
//! resolves through the same `0x76c700` but reports `"Couldn't find relative frame: %s"` at
//! severity 1 and **skips the anchor** (`0x767969 test ebx,ebx` → `0x76796d` → `jmp 0x7679dd`,
//! past the `0x7679c8 call 0x767c70`). That half lives in `crate::loader::geometry`, which
//! therefore resolves its own names and never reaches the ladder here.

use mlua::{Lua, MultiValue, Table, Value};

use crate::layout::Point;
use crate::script::{Model, SCREEN};

use super::{decode_id, point_from_str, prefetch_named_target, resolve_named_target, NamedTarget};

/// What the reference prints where a widget has no name — the literal `0x84c7f0`, reached from
/// `call [primary_vt+0x4]` (`GetName` → `[widget+0x98]`) returning NULL.
pub(crate) const UNNAMED: &str = "<unnamed>";

/// `relativeTo`, decoded — the four tags `lua_type(L,3)` accepts (`0x7a25e6`–`0x7a2620`); anything
/// else, **absent included** (`TNONE` = −1), is a Usage raise on the `SetPoint` side.
pub(crate) enum RelTarget {
    /// A widget wrapper table (`0x7a26c9`: `rawgeti(3,0)` → `touserdata` → `+0x24`).
    ///
    /// **The reference runs no `IsA` check on this arm** — unlike the `this` ladder and unlike the
    /// string arm, whose check lives inside `0x76c760` — so `add eax,0x24` is applied to whatever
    /// `lua_touserdata` returned and a table carrying any light userdata at `[0]` is accepted as a
    /// widget. That is a wild pointer, not a behaviour: [`resolve_rel_target`] takes the layout
    /// parent for a table that decodes to no live widget, and says so there.
    Wrapper(Table),
    /// A name string, already read out of `_G` with no `Model` guard alive ([`NamedTarget`]).
    Named(NamedTarget),
    /// An explicit `nil` — the **screen root** `ds:[0xcf0bd8]` (`0x7a2710`), *not* the parent. One
    /// stock site takes it (`UIParent.lua:1549`) and fourteen corpus sites do, every one of them
    /// meaning "the screen" (`pfUI`'s screenshot caption, `FixCTGroups`' cursor-space plate).
    Screen,
    /// A NUMBER in the `relativeTo` slot: `edi` stays 3 and the number is `offsetX` — the
    /// `SetPoint(point, x, y)` shorthand (`0x7a270e`). `relativeTo` keeps the layout parent's
    /// default (`0x76c6e0`: `[+0x9c]`'s layout sub-object, else the screen root).
    DefaultParent,
}

/// `SetPoint`'s parsed arguments (`0x7a2540`), ready for the caller's own commit.
pub(crate) struct PointArgs {
    pub(crate) point: Point,
    pub(crate) target: RelTarget,
    pub(crate) rel_point: Point,
    pub(crate) x: f32,
    pub(crate) y: f32,
}

/// `Usage: %s:SetPoint(…)` — `luaL_error(0x87cc28)`, the string read out of the reference image.
fn usage(who: &str) -> mlua::Error {
    mlua::Error::runtime(format!(
        "Usage: {who}:SetPoint(\"point\" [, region or nil] [, \"relativePoint\"] \
         [, offsetX, offsetY])"
    ))
}

/// `%s:SetPoint(): Unknown region point` — `0x87cd04`, raised by the 9-entry name scan `0x6f1840`
/// for either the `point` or a `relativePoint` that is a string it rejects.
fn unknown_point(who: &str) -> mlua::Error {
    mlua::Error::runtime(format!("{who}:SetPoint(): Unknown region point"))
}

/// `%s:%s(): Couldn't find region named '%s'` — `0x87ccd4` for `SetPoint`, `0x87cd84` for
/// `SetAllPoints`. **No fallback and no no-op**: this is the leg benilla answered with the parent
/// plus a warning until 2176.
pub(crate) fn no_such_region(who: &str, verb: &str, name: &str) -> mlua::Error {
    mlua::Error::runtime(format!(
        "{who}:{verb}(): Couldn't find region named '{name}'"
    ))
}

/// `%s:%s(): trying to anchor to itself` — `0x87cca8` / `0x87cd54`, the `[ebp-4] == this+0x24`
/// compare at `0x7a2764` / `0x7a2aa8`.
fn anchor_to_self(who: &str, verb: &str) -> mlua::Error {
    mlua::Error::runtime(format!("{who}:{verb}(): trying to anchor to itself"))
}

/// `lua_isnumber 0x6f34d0` — a number, or a string Lua can read as one.
fn as_number(lua: &Lua, v: &Value) -> Option<f32> {
    lua.coerce_number(v.clone())
        .ok()
        .flatten()
        .map(|n| n as f32)
}

/// `lua_isstring 0x6f3510` — tag 4 **or** 3, so a number is a string here.
fn as_string(lua: &Lua, v: &Value) -> Option<String> {
    match v {
        Value::String(_) | Value::Number(_) | Value::Integer(_) => lua
            .coerce_string(v.clone())
            .ok()
            .flatten()
            .and_then(|s| s.to_str().ok().map(|r| r.to_string())),
        _ => None,
    }
}

/// Parse `SetPoint(point [, relativeTo [, relativePoint]] [, x, y])` in the reference's own order:
/// the two tag gates first (both Usage), then the point-name scan, then `relativePoint`, then the
/// offsets. The order is observable — an unknown point with an *absent* `relativeTo` raises Usage,
/// not "Unknown region point".
///
/// `who` names the receiver for the error strings, and `parent_base` is what a leading `$parent`
/// expands to. Both are the caller's to compute, under a short `Model` borrow, **before** this
/// runs: the `_G` read here is a `lua_gettable` and an `__index` on `_G` can call straight back
/// into a binding ([`NamedTarget`]).
pub(crate) fn parse_set_point(
    lua: &Lua,
    args: &MultiValue,
    who: &str,
    parent_base: &str,
) -> mlua::Result<PointArgs> {
    // arg 2 = point. `lua_isstring(L,2)` false → `0x7a290d` Usage (`0x7a25d2`).
    let point_str = args.front().and_then(|v| as_string(lua, v));
    let Some(point_str) = point_str else {
        return Err(usage(who));
    };

    // arg 3 = relativeTo. `lua_type(L,3)` ∈ {4 STRING, 5 TABLE, 0 NIL, 3 NUMBER}, else Usage —
    // and an **absent** argument is `TNONE` = −1, which fails all four (`0x7a25e6`–`0x7a2620`).
    let Some(rel_arg) = args.get(1) else {
        return Err(usage(who));
    };
    let target = match rel_arg {
        Value::String(s) => RelTarget::Named(match s.to_str() {
            Ok(n) => prefetch_named_target(lua, n.as_ref(), Some(parent_base)),
            Err(_) => NamedTarget::unreadable(),
        }),
        Value::Table(t) => RelTarget::Wrapper(t.clone()),
        Value::Nil => RelTarget::Screen,
        Value::Number(_) | Value::Integer(_) => RelTarget::DefaultParent,
        _ => return Err(usage(who)),
    };

    // The point name, scanned by `0x6f1840` only now (`0x7a264a`) — after both tag gates.
    let point = point_from_str(&point_str).ok_or_else(|| unknown_point(who))?;

    // `relativePoint` is seeded from `point` (`0x7a2697`) and read at index `edi`, which the
    // number arm above left at 3 and every other arm advanced to 4. **Only a STRING is consumed**
    // (`0x7a2810 lua_type == 4`, strictly — not `lua_isstring`); anything else leaves the seed and
    // the offsets are taken from here.
    let mut cur = if matches!(target, RelTarget::DefaultParent) {
        1
    } else {
        2
    };
    let mut rel_point = point;
    if let Some(Value::String(s)) = args.get(cur) {
        let name = s.to_str()?;
        rel_point = point_from_str(name.as_ref()).ok_or_else(|| unknown_point(who))?;
        cur += 1;
    }

    // **Both offsets, or neither** (`0x7a2888 je 0x7a28e6`): each is gated on `lua_isnumber`, and
    // one failing zeroes the pair rather than only itself.
    let (x, y) = match (
        args.get(cur).and_then(|v| as_number(lua, v)),
        args.get(cur + 1).and_then(|v| as_number(lua, v)),
    ) {
        (Some(x), Some(y)) => (x, y),
        _ => (0.0, 0.0),
    };

    Ok(PointArgs {
        point,
        target,
        rel_point,
        x,
        y,
    })
}

/// Parse `SetAllPoints([relativeTo])` (`0x7a2940`) — **the same resolver, a different argument
/// law**, and the three differences are per-binding facts rather than a family rule:
///
/// 1. An **omitted** argument is legal: it falls through to the layout parent with no raise, where
///    `SetPoint` raises Usage.
/// 2. The name path is gated on **`lua_isstring`** (`0x7a29ea`), tag 4 **or** 3 — so
///    `f:SetAllPoints(42)` stringifies and looks up `_G["42"]`, where `SetPoint` tests
///    `lua_type == 4` strictly and routes a number to the offset shorthand.
/// 3. There is no `relativePoint` or offset parsing at all.
pub(crate) fn parse_set_all_points(lua: &Lua, arg: Option<&Value>, parent_base: &str) -> RelTarget {
    match arg {
        Some(Value::Table(t)) => RelTarget::Wrapper(t.clone()),
        Some(Value::Nil) => RelTarget::Screen,
        // `lua_isstring` — a NUMBER takes the name path here, unlike `SetPoint`.
        Some(v) => match as_string(lua, v) {
            Some(n) => RelTarget::Named(prefetch_named_target(lua, &n, Some(parent_base))),
            None => RelTarget::DefaultParent,
        },
        None => RelTarget::DefaultParent,
    }
}

/// Turn a parsed [`RelTarget`] into the layout id to anchor against, raising the reference's own
/// errors. `me` is the receiver's own id — the self-anchor compare (`0x7a2764` / `0x7a2aa8`).
///
/// The **wrapper** arm takes `default_parent` where the reference would take a wild pointer (see
/// [`RelTarget::Wrapper`]); every other arm is the reference's.
pub(crate) fn resolve_rel_target(
    model: &Model,
    target: &RelTarget,
    who: &str,
    verb: &str,
    me: u32,
    default_parent: u32,
) -> mlua::Result<u32> {
    let id = match target {
        RelTarget::Wrapper(t) => decode_id(t)
            .ok()
            .filter(|id| model.id_to_frame.contains_key(id) || model.id_to_region.contains_key(id))
            .unwrap_or(default_parent),
        RelTarget::Named(nt) => {
            resolve_named_target(model, nt).ok_or_else(|| no_such_region(who, verb, &nt.name))?
        }
        RelTarget::Screen => SCREEN,
        RelTarget::DefaultParent => default_parent,
    };
    if id == me {
        return Err(anchor_to_self(who, verb));
    }
    Ok(id)
}

/// The **XML** `<Anchor relativeTo="name">` resolution — `CLayoutFrame::LoadXML 0x767800`'s own,
/// which is a different law from the Lua ladder above and belongs to `crate::loader`.
///
/// `0x767964 call [eax+0x28]` is the same `0x76c700` the Lua string arm uses, so the *lookup* is
/// identical; what differs is the miss. `0x767969 test ebx,ebx` → `0x76796d` pushes
/// `0x878440 "Couldn't find relative frame: %s"` at severity 1 through the XML report's
/// `[ecx+0xc]` and then `jmp 0x7679dd`, which lands **past** the `0x7679c8 call 0x767c70` — the
/// anchor is reported and SKIPPED. It does not raise (there is no Lua state to unwind here) and it
/// does not fall back to the parent, which is what benilla did until 2176.
///
/// `$parent` is already expanded by the caller, so no base is taken.
pub(crate) fn resolve_xml_relative_to(lua: &Lua, name: &str) -> Option<Table> {
    // ONE `_G` read, and the wrapper it produced is the one handed back — re-reading `_G` to fetch
    // the table again would be a second `lua_gettable`, and an `__index` is allowed to answer
    // twice differently.
    let nt = prefetch_named_target(lua, name, None);
    let model = lua.app_data_ref::<Model>().expect("model");
    resolve_named_target(&model, &nt)?;
    // The wrapper the ladder's TABLE arm takes — resolved here so the binding never sees a name
    // from the XML path and so its raise can never reach a loading document.
    nt.wrapper.clone()
}
