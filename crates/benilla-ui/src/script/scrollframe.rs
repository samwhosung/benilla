//! The `ScrollFrame` methods (`CSimpleScrollFrame`): the scroll child, the two offsets, the two
//! ranges and `UpdateScrollChildRect`, over [`ScrollFrameState`]; the layout's `resolve`,
//! `extract` and `hit_test` place and clip the child. The axes are one mechanism: `0x786d30`
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

/// `Get{Vertical,Horizontal}ScrollRange()`: `max(0, content − frame)` from the resolved rects,
/// computed on every call, where the reference caches both (`[+0x31c]`, `[+0x320]`) and recomputes
/// them only once marked dirty. The content is the scroll child's whole subtree, not its own rect
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
            let changed = with_scroll(lua, &this, |s| {
                let changed = s.child != new_child;
                s.child = new_child;
                changed
            })?;
            if changed {
                // The child is an input of the layout resolve, so a real change dirties it.
                lua.app_data_mut::<Model>().expect("model").touch_layout();
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
            lua.app_data_mut::<Model>().expect("model").touch_layout();
            fire_scroll(lua, &this, "OnVerticalScroll", px)
        })?,
    )?;
    m.set(
        "GetVerticalScroll",
        lua.create_function(|lua, this: Table| with_scroll(lua, &this, |s| s.vertical))?,
    )?;
    m.set(
        "GetVerticalScrollRange",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(scroll_range(&model, h, Axis::Vertical))
        })?,
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
            lua.app_data_mut::<Model>().expect("model").touch_layout();
            fire_scroll(lua, &this, "OnHorizontalScroll", px)
        })?,
    )?;
    m.set(
        "GetHorizontalScroll",
        lua.create_function(|lua, this: Table| with_scroll(lua, &this, |s| s.horizontal))?,
    )?;
    m.set(
        "GetHorizontalScrollRange",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(scroll_range(&model, h, Axis::Horizontal))
        })?,
    )?;
    // UpdateScrollChildRect(): recomputes both ranges and fires `OnScrollRangeChanged` at once,
    // where the reference only marks them dirty (`0x791943`) for its next pump, which fires on a
    // change. A script resize reaches the resolved rects only at the app's next resolve, so this
    // resolves first.
    m.set(
        "UpdateScrollChildRect",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            // The reference measures text synchronously and ours lands a frame later, so the
            // child's FontStrings are measured first: `QuestLog_UpdateQuestDetails` calls this
            // right after its SetTexts (`QuestLogFrame.lua:458`).
            let strings: Vec<crate::widget::RegionHandle> = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                match model.arena.frame(h).map(|f| &f.kind_state) {
                    Some(KindState::Scroll(sf)) => sf
                        .child
                        .map(|c| subtree_font_strings(&model, c))
                        .unwrap_or_default(),
                    _ => Vec::new(),
                }
            };
            for rh in strings {
                super::measure::ensure_measured(lua, rh);
            }
            let (id, x_range, y_range) = {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                super::UiScript::resolve_layout(&mut model);
                let x_range = scroll_range(&model, h, Axis::Horizontal);
                let y_range = scroll_range(&model, h, Axis::Vertical);
                (model.frame_id(h), x_range, y_range)
            };
            // The resolve can move sizes, and the reference fires `OnSizeChanged` during layout, so
            // those fire before the range notify.
            event::fire_size_changes(lua);
            // `OnScrollRangeChanged(self, xRange, yRange)`: horizontal first (`0x786eff`), which is
            // why stock passes `arg2` on as the vertical range (`UIPanelTemplates.xml:188`).
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
                    .record_script_error(e.to_string());
            }
            Ok(())
        })?,
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
            .record_script_error(e.to_string());
    }
    Ok(())
}
