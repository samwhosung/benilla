//! The `ScrollFrame` methods (`CSimpleScrollFrame`): the scroll child, the two offsets, the two
//! ranges and `UpdateScrollChildRect`, over [`ScrollFrameState`], and the frame's own update
//! ([`update`]), which keeps the ranges; the layout's `resolve`, `extract` and `hit_test` place
//! and clip the child. The axes are one mechanism: `0x786d30`
//! (horizontal) is a byte-clone of `0x786db0` (vertical) on `[+0x324]`/`[+0x32c]` for
//! `[+0x328]`/`[+0x334]`, and `0x786e30` measures both ranges in one pass. The methods sit in
//! their own table, so on every other frame kind `SetScrollChild` is nil, as in the client.

use mlua::{Lua, Table, Value};

use super::object::{decode_id, frame_handle_of, frame_wrapper};
use super::{event, Model};
use crate::widget::{FrameHandle, KindState, ScrollFrameState};

/// Registry key of the ScrollFrame method table.
pub(super) const REG_SCROLLFRAME_METHODS: &str = "__benilla_scrollframe_methods";

/// Run `f` over a frame's ScrollFrame state; errors when `this` is not a live ScrollFrame, which a
/// script reaches only by applying a method taken off the table to another frame.
fn with_scroll<T>(
    lua: &Lua,
    this: &Table,
    f: impl FnOnce(&mut ScrollFrameState) -> T,
) -> mlua::Result<T> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let frame = model
        .arena
        .frame_mut(h)
        .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
    match &mut frame.kind_state {
        KindState::Scroll(s) => Ok(f(s)),
        _ => Err(mlua::Error::runtime("not a ScrollFrame")),
    }
}

/// The axis of an offset or a range. `0x786e30` computes both ranges in one pass over one subtree
/// union: `[+0x31c] = max(0, maxRIGHT − minLEFT − width) / scale`, and `[+0x320]` the same with
/// top, bottom and height.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Axis {
    Horizontal,
    Vertical,
}

/// One range as the recompute `0x786e30` measures it: `max(0, content − frame)` from the resolved
/// rects. The content is the scroll child's whole subtree, not its own rect
/// (`0x786e30` calls the recursive `0x786f80`): stock `ItemTextPageScrollChild` is 10×10 around a
/// 270×304 SimpleHTML (`ItemTextFrame.xml:198`). Each extent is divided by its frame's effective
/// scale, since the offset is in local units; a range in screen px under-scrolls a scaled frame.
fn scroll_range(model: &Model, h: FrameHandle, axis: Axis) -> f32 {
    let child = match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(KindState::Scroll(s)) => s.child,
        _ => None,
    };
    let Some(child) = child else { return 0.0 };
    let scale_of = |f: FrameHandle| {
        model
            .arena
            .frame(f)
            .map(|x| x.effective_scale)
            .filter(|s| s.abs() >= 1e-6)
            .unwrap_or(1.0)
    };
    let Some(viewport) = model.resolved.get(&h).map(|r| {
        let extent = match axis {
            Axis::Horizontal => r.width(),
            Axis::Vertical => r.height(),
        };
        extent / scale_of(h)
    }) else {
        return 0.0;
    };
    let Some((far, near)) = subtree_span(model, child, axis) else {
        return 0.0;
    };
    ((far - near) / scale_of(child) - viewport).max(0.0)
}

/// The span of `f`'s subtree along `axis` in screen px, as `(far, near)`: its own rect unioned with
/// its shown regions and visible child frames, recursively (`0x786f80`). Hidden parts add no range:
/// the client guards both lists on visibility.
fn subtree_span(model: &Model, f: FrameHandle, axis: Axis) -> Option<(f32, f32)> {
    let edges = |r: &crate::layout::Rect| match axis {
        Axis::Horizontal => (r.right, r.left),
        Axis::Vertical => (r.top, r.bottom),
    };
    let frame = model.arena.frame(f)?;
    let mut span: Option<(f32, f32)> = model.resolved.get(&f).map(edges);
    let union = |s: &mut Option<(f32, f32)>, far: f32, near: f32| {
        *s = Some(match *s {
            Some((of, on)) => (of.max(far), on.min(near)),
            None => (far, near),
        });
    };
    for &rh in &frame.regions {
        if model.region_data.get(&rh).is_some_and(|d| d.hidden) {
            continue;
        }
        if let Some(r) = model.region_resolved.get(&rh) {
            let (far, near) = edges(r);
            union(&mut span, far, near);
        }
    }
    for &ch in &frame.children {
        if !model.arena.frame(ch).is_some_and(|c| c.effective_visible) {
            continue;
        }
        if let Some((far, near)) = subtree_span(model, ch, axis) {
            union(&mut span, far, near);
        }
    }
    span
}

/// Every FontString under `f` (its own regions, then its visible children's), in tree order.
fn subtree_font_strings(model: &Model, f: FrameHandle) -> Vec<crate::widget::RegionHandle> {
    let mut out = Vec::new();
    let Some(frame) = model.arena.frame(f) else {
        return out;
    };
    for &rh in &frame.regions {
        if model
            .arena
            .region(rh)
            .is_some_and(|r| matches!(r.kind, crate::widget::RegionKind::FontString))
        {
            out.push(rh);
        }
    }
    for &ch in &frame.children {
        if model.arena.frame(ch).is_some_and(|c| c.effective_visible) {
            out.extend(subtree_font_strings(model, ch));
        }
    }
    out
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    // SetScrollChild(frame | name | nil): nil or an unresolvable target clears.
    m.set(
        "SetScrollChild",
        lua.create_function(|lua, (this, target): (Table, Value)| {
            // A name is `_G[name]` with no `$parent`: `0x790fa0` calls `SetParent`'s resolver
            // `0x76c760`.
            let named = match &target {
                Value::String(s) => s
                    .to_str()
                    .ok()
                    .map(|n| super::object::prefetch_named_target(lua, n.as_ref(), None)),
                _ => None,
            };
            let new_child: Option<FrameHandle> = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                match &target {
                    Value::Table(t) => decode_id(t)
                        .ok()
                        .and_then(|id| model.id_to_frame.get(&id).copied()),
                    Value::String(_) => named.as_ref().and_then(|nt| {
                        super::object::resolve_named_target(&model, nt)
                            .and_then(|id| model.id_to_frame.get(&id).copied())
                    }),
                    _ => None,
                }
            };
            // Every call marks the range (`0x786d16`), the same child again included.
            let old_child = with_scroll(lua, &this, |s| {
                s.range_dirty = true;
                std::mem::replace(&mut s.child, new_child)
            })?;
            if old_child != new_child {
                // The child takes this frame's override anchor and the old one drops it; the
                // ScrollFrame's node re-derives its new child with it.
                let h = frame_handle_of(lua, &this)?;
                let mut model = lua.app_data_mut::<Model>().expect("model");
                model.touch_layout_frame(h);
                if let Some(old) = old_child {
                    model.touch_layout_frame(old);
                }
            }
            Ok(())
        })?,
    )?;
    m.set(
        "GetScrollChild",
        lua.create_function(|lua, this: Table| {
            let child = with_scroll(lua, &this, |s| s.child)?;
            let id = {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                child
                    .filter(|&h| model.arena.frame(h).is_some())
                    .map(|h| model.frame_id(h))
            };
            match id {
                Some(id) => Ok(Value::Table(frame_wrapper(lua, id)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // SetVerticalScroll(px) (`0x786db0`): stores the offset unclamped (`[+0x328]`), re-anchors the
    // child (`0x787100`) and fires `OnVerticalScroll(self, offset)`, all only on a change; the
    // reference skips a change under 2⁻²² from the old offset, where this compares bits. The range
    // is never read here: every clamp is FrameXML's, through the scroll bar's min and max.
    m.set(
        "SetVerticalScroll",
        lua.create_function(|lua, (this, px): (Table, f32)| {
            let changed = with_scroll(lua, &this, |s| {
                let changed = s.vertical.to_bits() != px.to_bits();
                s.vertical = px;
                changed
            })?;
            if !changed {
                return Ok(());
            }
            // The child's override offset: the ScrollFrame's node re-derives its child.
            let h = frame_handle_of(lua, &this)?;
            lua.app_data_mut::<Model>()
                .expect("model")
                .touch_layout_frame(h);
            fire_scroll(lua, &this, "OnVerticalScroll", px)
        })?,
    )?;
    m.set(
        "GetVerticalScroll",
        lua.create_function(|lua, this: Table| with_scroll(lua, &this, |s| s.vertical))?,
    )?;
    // The stored range (`0x7917d0` reads `[+0x320]` at `0x791873`), stale while the frame is
    // hidden or its range unmarked.
    m.set(
        "GetVerticalScrollRange",
        lua.create_function(|lua, this: Table| with_scroll(lua, &this, |s| s.range[1]))?,
    )?;

    // ── The horizontal axis ──
    // SetHorizontalScroll(px) (`0x7912f0` → `0x786d30`) is the vertical setter's byte-clone on
    // `[+0x324]`, firing `OnHorizontalScroll`. The sign is the reference's: `0x787100` anchors the
    // child at `(+hScroll, +vScroll)`, so a positive x moves it right, where a positive y lifts it.
    m.set(
        "SetHorizontalScroll",
        lua.create_function(|lua, (this, px): (Table, f32)| {
            let changed = with_scroll(lua, &this, |s| {
                let changed = s.horizontal.to_bits() != px.to_bits();
                s.horizontal = px;
                changed
            })?;
            if !changed {
                return Ok(());
            }
            let h = frame_handle_of(lua, &this)?;
            lua.app_data_mut::<Model>()
                .expect("model")
                .touch_layout_frame(h);
            fire_scroll(lua, &this, "OnHorizontalScroll", px)
        })?,
    )?;
    m.set(
        "GetHorizontalScroll",
        lua.create_function(|lua, this: Table| with_scroll(lua, &this, |s| s.horizontal))?,
    )?;
    m.set(
        "GetHorizontalScrollRange",
        lua.create_function(|lua, this: Table| with_scroll(lua, &this, |s| s.range[0]))?,
    )?;
    // UpdateScrollChildRect() (`0x7918b0`) marks the range and does nothing else (`0x791943`):
    // the frame's next update while shown recomputes it.
    m.set(
        "UpdateScrollChildRect",
        lua.create_function(|lua, this: Table| with_scroll(lua, &this, |s| s.range_dirty = true))?,
    )?;

    lua.set_named_registry_value(REG_SCROLLFRAME_METHODS, m)?;
    Ok(())
}

/// Fire `On{Vertical,Horizontal}Scroll(self, offset)`, the reference's script slots `[+0x334]` and
/// `[+0x32c]`, outside any model borrow.
fn fire_scroll(lua: &Lua, this: &Table, script: &str, value: f32) -> mlua::Result<()> {
    let id = {
        let h = frame_handle_of(lua, this)?;
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        model.frame_id(h)
    };
    if let Err(e) =
        event::fire_widget_handler(lua, id, script, vec![Value::Number(f64::from(value))])
    {
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .record_lua_error(&e);
    }
    Ok(())
}

/// The frame's update (`0x787140`, the walk's slot after its `OnUpdate`), so only while shown. A
/// resize since the last one marks the range, as the size slot `0x7871f0` does (`0x787205`). A
/// marked range is recomputed (`0x787152`–`0x7871a0`, `0x786e30`) unless the frame has no rect
/// (`0x787186`) or no scroll child (`0x786e42`), and the mark is cleared either way, after the
/// notify (`0x7871a5`). The recompute stores both ranges (`0x786eaa`, `0x786ed1`) and fires
/// `OnScrollRangeChanged(self, xRange, yRange)` when either moved by 2⁻²² (`[0x8029d4]`) or more
/// in the reference's units (the screen diagonal as 1) (`0x786ed7`–`0x786efd`), which is that times the
/// layout unit in ours.
///
/// The reference drains its layout before the walk (`0x7656ba`) and measures text at once; ours
/// lands both a frame later, so a marked frame measures its child's FontStrings and resolves first
/// (`QuestLog_UpdateQuestDetails` marks right after its `SetText`s, `QuestLogFrame.lua:458`). An
/// unmarked frame reads its size as the last resolve left it, so a resize not yet resolved marks
/// it at the update after that resolve.
pub(super) fn update(lua: &Lua, h: FrameHandle) {
    let marked = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let rect = model.resolved.get(&h).copied();
        let Some(s) = scroll_state(&mut model, h) else {
            return;
        };
        see_size(s, rect);
        s.range_dirty
    };
    if !marked {
        return;
    }
    let strings = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        match model.arena.frame(h).map(|f| &f.kind_state) {
            Some(KindState::Scroll(s)) => s
                .child
                .map(|c| subtree_font_strings(&model, c))
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    };
    for rh in strings {
        super::measure::ensure_measured(lua, rh);
    }
    super::UiScript::resolve_layout(&mut lua.app_data_mut::<Model>().expect("model app_data"));
    // The reference fires `OnSizeChanged` from its layout drain, ahead of the walk.
    event::fire_size_changes(lua);
    let notify = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let rect = model.resolved.get(&h).copied();
        let has_child = matches!(
            model.arena.frame(h).map(|f| &f.kind_state),
            Some(KindState::Scroll(s)) if s.child.is_some()
        );
        let measured = (rect.is_some() && has_child).then(|| {
            [
                scroll_range(&model, h, Axis::Horizontal),
                scroll_range(&model, h, Axis::Vertical),
            ]
        });
        let eps = crate::layout::SIZE_EPS * f64::from(model.layout_unit());
        let Some(s) = scroll_state(&mut model, h) else {
            return;
        };
        // The resolve may have moved the frame: that is the size this update has seen.
        see_size(s, rect);
        let moved = measured.filter(|new| {
            let old = std::mem::replace(&mut s.range, *new);
            (0..2).any(|i| (f64::from(new[i]) - f64::from(old[i])).abs() >= eps)
        });
        if moved.is_none() {
            s.range_dirty = false;
        }
        moved.map(|range| (model.frame_id(h), range))
    };
    let Some((id, [x_range, y_range])) = notify else {
        return;
    };
    // Horizontal first (`0x786eff`), which is why stock passes `arg2` on as the vertical range
    // (`UIPanelTemplates.xml:188`).
    if let Err(e) = event::fire_widget_handler(
        lua,
        id,
        "OnScrollRangeChanged",
        vec![
            Value::Number(f64::from(x_range)),
            Value::Number(f64::from(y_range)),
        ],
    ) {
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .record_lua_error(&e);
    }
    // The mark clears after the notify, so one the handler set is dropped.
    if let Some(s) = scroll_state(&mut lua.app_data_mut::<Model>().expect("model app_data"), h) {
        s.range_dirty = false;
    }
}

/// Frame `h`'s ScrollFrame state, if it is a live ScrollFrame.
fn scroll_state(model: &mut Model, h: FrameHandle) -> Option<&mut ScrollFrameState> {
    match model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
        Some(KindState::Scroll(s)) => Some(s),
        _ => None,
    }
}

/// Note the frame's resolved size; a change since the last note marks the range, by the width or
/// height move that fires `OnSizeChanged` ([`crate::layout::size_changed`]). No rect, no note.
fn see_size(s: &mut ScrollFrameState, rect: Option<crate::layout::Rect>) {
    let Some(r) = rect else { return };
    let size = [r.width(), r.height()];
    let moved = |i: usize| {
        (f64::from(size[i]) - f64::from(s.seen_size[i])).abs() >= crate::layout::SIZE_EPS
    };
    if moved(0) || moved(1) {
        s.range_dirty = true;
    }
    s.seen_size = size;
}
