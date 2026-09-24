//! The argument ladder of `SetPoint` (`0x7a2540`) and `SetAllPoints` (`0x7a2940`): the reference
//! registers both once, in the Region method table (`0x87c9b8`), so every widget type runs the
//! same function. Every failure leg raises; none falls back to the parent or does nothing.

use mlua::{Lua, MultiValue, Table, Value};

use crate::layout::Point;
use crate::script::{Model, SCREEN};

use super::{decode_id, point_from_str, prefetch_named_target, resolve_named_target, NamedTarget};

/// The name the reference prints for a widget that has none (the literal at `0x84c7f0`).
pub(crate) const UNNAMED: &str = "<unnamed>";

/// `relativeTo` by the four tags `lua_type(L,3)` accepts (`0x7a25e6`..`0x7a2620`); any other tag,
/// absent included, raises Usage in `SetPoint`.
pub(crate) enum RelTarget {
    /// A widget wrapper table (`0x7a26c9`); the reference runs no `IsA` check on it.
    Wrapper(Table),
    /// A name string, already read out of `_G` with no `Model` guard alive ([`NamedTarget`]).
    Named(NamedTarget),
    /// An explicit `nil`: the screen root `ds:[0xcf0bd8]` (`0x7a2710`), not the parent.
    Screen,
    /// A number: the `SetPoint(point, x, y)` shorthand (`0x7a270e`), against the default parent.
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

/// The reference's `Usage:` raise (`0x87cc28`).
fn usage(who: &str) -> mlua::Error {
    mlua::Error::runtime(format!(
        "Usage: {who}:SetPoint(\"point\" [, region or nil] [, \"relativePoint\"] \
         [, offsetX, offsetY])"
    ))
}

/// `0x87cd04`: the point-name scan `0x6f1840` rejected `point` or a string `relativePoint`.
fn unknown_point(who: &str) -> mlua::Error {
    mlua::Error::runtime(format!("{who}:SetPoint(): Unknown region point"))
}

/// `0x87ccd4` for `SetPoint`, `0x87cd84` for `SetAllPoints`: a raise, never a fallback.
pub(crate) fn no_such_region(who: &str, verb: &str, name: &str) -> mlua::Error {
    mlua::Error::runtime(format!(
        "{who}:{verb}(): Couldn't find region named '{name}'"
    ))
}

/// `0x87cca8` / `0x87cd54`, from the self compare at `0x7a2764` / `0x7a2aa8`.
fn anchor_to_self(who: &str, verb: &str) -> mlua::Error {
    mlua::Error::runtime(format!("{who}:{verb}(): trying to anchor to itself"))
}

/// `lua_isnumber` (`0x6f34d0`): a number, or a string Lua reads as one.
fn as_number(lua: &Lua, v: &Value) -> Option<f32> {
    lua.coerce_number(v.clone())
        .ok()
        .flatten()
        .map(|n| n as f32)
}

/// `lua_isstring` (`0x6f3510`): a string or a number.
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

/// Parse `SetPoint` in the reference's order: both tag gates (Usage), the point-name scan,
/// `relativePoint`, the offsets; so an unknown point with no `relativeTo` raises Usage. The caller
/// computes `who` and `parent_base` first, since an `__index` on `_G` can call back into a binding.
pub(crate) fn parse_set_point(
    lua: &Lua,
    args: &MultiValue,
    who: &str,
    parent_base: &str,
) -> mlua::Result<PointArgs> {
    let point_str = args.front().and_then(|v| as_string(lua, v));
    let Some(point_str) = point_str else {
        return Err(usage(who));
    };

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

    let point = point_from_str(&point_str).ok_or_else(|| unknown_point(who))?;

    // `relativePoint` defaults to `point` (`0x7a2697`) and is consumed only when strictly a string
    // (`0x7a2810`), else the offsets start here; the number arm reads one slot earlier.
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

    // Both offsets or neither: one failing `lua_isnumber` zeroes both (`0x7a2888` → `0x7a28e6`).
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

/// Parse `SetAllPoints([relativeTo])` (`0x7a2940`). Unlike `SetPoint`, an omitted argument takes
/// the layout parent, and a number is a name (`lua_isstring`, `0x7a29ea`): `SetAllPoints(42)`
/// looks up `_G["42"]`.
pub(crate) fn parse_set_all_points(lua: &Lua, arg: Option<&Value>, parent_base: &str) -> RelTarget {
    match arg {
        Some(Value::Table(t)) => RelTarget::Wrapper(t.clone()),
        Some(Value::Nil) => RelTarget::Screen,
        Some(v) => match as_string(lua, v) {
            Some(n) => RelTarget::Named(prefetch_named_target(lua, &n, Some(parent_base))),
            None => RelTarget::DefaultParent,
        },
        None => RelTarget::DefaultParent,
    }
}

/// Resolve a [`RelTarget`] to the layout id to anchor to, raising the reference's errors; `me` is
/// the receiver, for the self-anchor check. Deviation: a wrapper that decodes to no live widget
/// takes `default_parent`, because the reference, with no `IsA` check, reads a wild pointer.
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

/// The XML `<Anchor relativeTo>` lookup (`CLayoutFrame::LoadXML 0x767800`), `$parent` already
/// expanded: the Lua string arm's resolver (`0x76c700`), but a miss reports "Couldn't find
/// relative frame" (`0x878440`) and skips the anchor (`0x767969`) without raising or falling back.
pub(crate) fn resolve_xml_relative_to(lua: &Lua, name: &str) -> Option<Table> {
    // One `_G` read, its wrapper returned: an `__index` may answer a second read differently.
    let nt = prefetch_named_target(lua, name, None);
    let model = lua.app_data_ref::<Model>().expect("model");
    resolve_named_target(&model, &nt)?;
    // A wrapper, not a name, so the Lua ladder's raise can never reach a loading document.
    nt.wrapper.clone()
}
