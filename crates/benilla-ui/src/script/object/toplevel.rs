//! Frame methods: the `toplevel` flag, `Raise` and `Lower`, and the raise every trigger runs.
//!
//! The flag is bit `0x1` of `[frame+0xb4]`, set by `0x76a3c0` from XML (`0x7698ec`) or
//! `SetToplevel` (`0x775440`), and setting it raises nothing. `CSimpleTop::Raise` (`0x7650f0`):
//!
//! 1. Walks up `+0x9c` to the nearest toplevel self-or-ancestor; with none it does nothing.
//! 2. With `force`, resolves the frame's layout and tests whether it is overlapped: a visible frame
//!    of its stratum at its level or above, neither itself nor a descendant, meets its rect.
//! 3. Only then compacts the stratum's levels below its high-water bound, hidden frames among
//!    them (`0x764eb0`), and sets its level to the bound, moving same-stratum children by the same
//!    delta (`0x76a4f0`).
//!
//! The stratum never changes. Step 2 stores what it found as the overlap bit `0x10` (`0x7651a3`),
//! which the draw pass also recomputes for every shown toplevel frame of a stratum whose levels
//! went dirty (`0x7657d0` → `0x765a90`). The unchecked raise (`force = 0`) skips step 2 and acts
//! on the stored bit (`0x7651ac`): a frame's rect change raises the frame a move or size holds
//! with it (`0x764a20` → `0x764a4c`), so a dragged window rises once it has been found
//! overlapped. The checked triggers are `Raise` (`0x775b0a`), Show (`0x76aeec`), drag start
//! (`0x7652d7`) and mouse-down (`0x766392`).

use mlua::{Lua, MultiValue, Table};

use crate::layout::Rect;
use crate::script::{Model, UiScript};
use crate::widget::FrameHandle;

use super::frame_handle_of;

/// Populate `m`'s toplevel and raise methods.
pub(super) fn install(lua: &Lua, m: &Table) -> mlua::Result<()> {
    // SetToplevel's argument goes through `GetBoolOrDefault` (`0x6f1c10`) with default 1
    // (`0x775440`, `0x7754ce`): a bare call sets it, an explicit nil clears it.
    m.set(
        "SetToplevel",
        lua.create_function(|lua, (this, args): (Table, MultiValue)| {
            let on = crate::script::binding_abi::bool_or_default(args.front(), true);
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
    let found = overlapped(model, t);
    if let Some(f) = model.arena.frame_mut(t) {
        f.overlapped = found;
    }
    if found {
        lift(model, t);
    }
    true
}

/// `CSimpleTop::Raise(frame, force = 0)` (`0x7650f0` by `0x764a4c`): step 1, then step 3 only when
/// the toplevel frame's stored overlap bit is set (`0x7651ac`).
pub(in crate::script) fn raise_unchecked(model: &mut Model, frame: FrameHandle) {
    let Some(t) = nearest_toplevel(model, frame) else {
        return;
    };
    if model.arena.frame(t).is_some_and(|f| f.overlapped) {
        lift(model, t);
    }
}

/// Step 3: compact the occupied levels below the stratum's bound (`0x764eb0`), then land on the
/// bound with the children following (`0x76a4f0(bound, 1)`, `0x7651d8`).
fn lift(model: &mut Model, t: FrameHandle) {
    let Some(strata) = model.arena.frame(t).map(|f| f.strata) else {
        return;
    };
    let count = model.arena.compact_levels(strata);
    model.arena.set_frame_level(t, count, true);
}

/// The draw pass's first loop (`0x7657d0`): for each stratum whose levels are dirty, every shown
/// toplevel frame's overlap bit takes `0x765a90`'s answer on the rects as they stand
/// (`0x765847`, `0x765851`), and the stratum's flag clears (`0x765872`).
pub(in crate::script) fn refresh_overlap_bits(model: &mut Model) {
    let dirty = model.arena.take_levels_dirty();
    if dirty == 0 {
        return;
    }
    let in_dirty = |f: &crate::widget::Frame| dirty & (1 << f.strata.index()) != 0;
    let tops: Vec<FrameHandle> = model
        .arena
        .iter_frames()
        .filter(|(_, f)| f.toplevel && f.effective_visible && in_dirty(f))
        .map(|(h, _)| h)
        .collect();
    if tops.is_empty() {
        return;
    }
    // The shown frames of the dirty strata with a rect, gathered once for every toplevel's scan.
    let shown: Vec<(FrameHandle, crate::order::Strata, u16, Rect)> = model
        .arena
        .iter_frames()
        .filter(|(_, f)| f.effective_visible && in_dirty(f))
        .filter_map(|(h, f)| Some((h, f.strata, f.level, *model.frame_rects.get(&h)?)))
        .collect();
    for t in tops {
        let found = model.frame_rects.get(&t).is_some_and(|&rect| {
            let tf = model.arena.frame(t).expect("listed above");
            shown.iter().any(|&(h, strata, level, r)| {
                h != t
                    && strata == tf.strata
                    && level >= tf.level
                    && rects_overlap(rect, r)
                    && !model.arena.is_ancestor(t, h)
            })
        });
        if let Some(f) = model.arena.frame_mut(t) {
            f.overlapped = found;
        }
    }
}

/// The Show trigger (`0x76ae10` at `0x76aee0`): on becoming visible, raise only if the frame's own
/// toplevel bit is set, so showing a child never lifts its window.
pub(in crate::script) fn raise_on_show(model: &mut Model, h: FrameHandle) {
    if model.arena.is_toplevel(h) {
        raise(model, h);
    }
}

/// Step 1: the first frame up `+0x9c` with the toplevel bit (`0x7650fb..0x765135`), the walk the
/// layout drag's lift repeats (`0x766fe0`).
pub(in crate::script) fn nearest_toplevel(
    model: &Model,
    frame: FrameHandle,
) -> Option<FrameHandle> {
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
    let Some(t_rect) = model.frame_rect(t) else {
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
                .frame_rect(h)
                .is_some_and(|r| rects_overlap(t_rect, r))
    })
}

/// Whether two rects intersect with area: the reference reads both (`0x768320`), intersects them
/// (`0x766c70`) and tests the result (`0x766bc0`).
fn rects_overlap(a: Rect, b: Rect) -> bool {
    let i = crate::script::clip::intersect_rect(a, b);
    i.width() > 0.0 && i.height() > 0.0
}
