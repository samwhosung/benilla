//! Frame methods: the `toplevel` flag, `Raise` and `Lower`, and the raise every trigger runs.
//!
//! The flag is bit `0x1` of `[frame+0xb4]`, set by `0x76a3c0` from XML (`0x7698ec`) or
//! `SetToplevel` (`0x775440`), and setting it raises nothing. `CSimpleTop::Raise` (`0x7650f0`):
//!
//! 1. Walks up `+0x9c` to the nearest toplevel self-or-ancestor; with none it does nothing.
//! 2. With `force`, resolves the frame's layout and tests whether it is overlapped: a visible frame
//!    of its stratum at its level or above, neither itself nor a descendant, meets its rect.
//! 3. Only then compacts the stratum's levels, hidden frames included (`0x764eb0`), and sets its
//!    level to the top plus one, moving same-stratum children by the same delta (`0x76a4f0`).
//!
//! The stratum never changes. The overlapped bit is recomputed per raise, never stored: only the
//! `force = 0` trigger (`0x764a4c`) reads a stored bit, and it is not wired, so neither is the
//! per-tick pass that keeps it (`0x7657d0`). The wired triggers are `Raise` (`0x775b0a`), Show
//! (`0x76aeec`), drag start (`0x7652d7`) and mouse-down (`0x766392`).

use mlua::{Lua, Table, Value};

use crate::layout::Rect;
use crate::script::{Model, UiScript};
use crate::widget::FrameHandle;

use super::frame_handle_of;

/// Populate `m`'s toplevel and raise methods.
pub(super) fn install(lua: &Lua, m: &Table) -> mlua::Result<()> {
    // SetToplevel's argument is optional and defaults to true (`0x775440`), unlike `SetMovable`'s.
    m.set(
        "SetToplevel",
        lua.create_function(|lua, (this, flag): (Table, Option<Value>)| {
            let on = match flag {
                None => true,
                Some(v) => !matches!(v, Value::Nil | Value::Boolean(false)),
            };
            let h = frame_handle_of(lua, &this)?;
            lua.app_data_mut::<Model>()
                .expect("model")
                .arena
                .set_toplevel(h, on);
            Ok(())
        })?,
    )?;
    m.set(
        "IsToplevel",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(crate::script::binding_abi::flag(model.arena.is_toplevel(h)))
        })?,
    )?;
    // Raise() (`0x775a50` → `0x76a5b0` → `0x7650f0`, `force = 1`): legal on any frame, and silent
    // when no toplevel self-or-ancestor exists.
    m.set(
        "Raise",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            raise(&mut model, h);
            Ok(())
        })?,
    )?;
    // Lower() does nothing in the reference: `0x775b20` → `0x76a5c0` → `0x7652a0`, which is
    // `xor eax,eax; ret 4`.
    m.set("Lower", lua.create_function(|_, _this: Table| Ok(()))?)?;
    Ok(())
}

/// `CSimpleTop::Raise(frame, force = 1)` (`0x7650f0`), the module doc's three steps; true, as in
/// the reference, when a toplevel self-or-ancestor was found, whether or not it moved.
pub(in crate::script) fn raise(model: &mut Model, frame: FrameHandle) -> bool {
    // 1. The nearest toplevel self-or-ancestor, else nothing.
    let Some(t) = nearest_toplevel(model, frame) else {
        return false;
    };
    // 2. Resolve the pending layout (`0x76513e`), then test for overlap. Deviation: an
    //    `OnSizeChanged` that resolve produces fires at the next `UiScript::resolve`, because a
    //    Lua handler must not run in the middle of a level compaction.
    UiScript::resolve_layout(model);
    if !overlapped(model, t) {
        return true;
    }
    // 3. The raise: compact the stratum's occupied levels, then land one above the top.
    let Some(strata) = model.arena.frame(t).map(|f| f.strata) else {
        return true;
    };
    let count = model.arena.compact_levels(strata);
    model.arena.set_frame_level(t, count, true);
    true
}

/// The Show trigger (`0x76ae10` at `0x76aee0`): on becoming visible, raise only if the frame's own
/// toplevel bit is set, so showing a child never lifts its window.
pub(in crate::script) fn raise_on_show(model: &mut Model, h: FrameHandle) {
    if model.arena.is_toplevel(h) {
        raise(model, h);
    }
}

/// Step 1: the first frame up `+0x9c` with the toplevel bit (`0x7650fb..0x765135`).
fn nearest_toplevel(model: &Model, frame: FrameHandle) -> Option<FrameHandle> {
    // A chain longer than the frame count is a cycle, which `SetParent` already refuses.
    let bound = model.arena.iter_frames().count();
    let mut cur = Some(frame);
    let mut guard = 0usize;
    while let Some(h) = cur {
        let f = model.arena.frame(h)?;
        if f.toplevel {
            return Some(h);
        }
        cur = f.parent;
        guard += 1;
        if guard > bound {
            break;
        }
    }
    None
}

/// Step 2 (`0x7650f0` inline, `0x765a90` standalone): a visible frame of `t`'s stratum at `t`'s
/// level or above, neither `t` nor its descendant (`0x767010`), whose rect meets `t`'s with area.
fn overlapped(model: &Model, t: FrameHandle) -> bool {
    let Some(tf) = model.arena.frame(t) else {
        return false;
    };
    let (strata, level) = (tf.strata, tf.level);
    let Some(t_rect) = model.resolved.get(&t).copied() else {
        // No resolved rect: not on screen, so nothing overlaps it.
        return false;
    };
    model.arena.iter_frames().any(|(h, f)| {
        h != t
            && f.effective_visible
            && f.strata == strata
            && f.level >= level
            && !model.arena.is_ancestor(t, h)
            && model
                .resolved
                .get(&h)
                .is_some_and(|r| rects_overlap(t_rect, *r))
    })
}

/// Whether two rects intersect with area: the reference reads both (`0x768320`), intersects them
/// (`0x766c70`) and tests the result (`0x766bc0`).
fn rects_overlap(a: Rect, b: Rect) -> bool {
    let i = crate::script::clip::intersect_rect(a, b);
    i.width() > 0.0 && i.height() > 0.0
}
