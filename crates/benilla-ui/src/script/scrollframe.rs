//! The `ScrollFrame` methods (`CSimpleScrollFrame`): the scroll child, the two offsets, the two
//! ranges and `UpdateScrollChildRect`, over [`ScrollFrameState`], and the frame's own update
//! ([`update`]), which keeps the ranges; the layout's `resolve`, `extract` and `hit_test` place
//! and clip the child. The axes are one mechanism: `0x786d30`
//! (horizontal) is a byte-clone of `0x786db0` (vertical) on `[+0x324]`/`[+0x32c]` for
//! `[+0x328]`/`[+0x334]`, and `0x786e30` measures both ranges in one pass. The methods sit in
//! their own table, so on every other frame kind `SetScrollChild` is nil, as in the client.

use mlua::{Lua, Table, Value};

use super::binding_abi;
use super::object::{decode_id, frame_handle_of, frame_wrapper};
use super::{event, Model};
use crate::layout::SIZE_EPS;
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

/// Both ranges as the recompute `0x786e30` measures them in one pass, `[horizontal, vertical]`.
/// The content box is the scroll child's whole subtree, not its own rect (`0x786e30` calls the
/// recursive `0x786f80`): stock `ItemTextPageScrollChild` is 10×10 around a 270×304 SimpleHTML
/// (`ItemTextFrame.xml:198`). Its left and bottom start at `f32::MAX` and its right and top at 0
/// (`0x786e5d`, `0x786e64`). Each range is the box's extent less the frame's (`0x786e84`), raised to 0
/// when not above it, NaN included (`0x786e8f`), times the reciprocal of the scroll frame's own
/// effective scale (`+0x7c`): at full precision for the horizontal (`0x786e9f`-`0x786eaa`), rounded
/// to f32 for the vertical (`0x786ea5`, `0x786ece`). The child's own scale is never read. All of it
/// is in layout units, the rects' own, as the ranges are stored.
fn scroll_ranges(model: &Model, h: FrameHandle) -> [f32; 2] {
    let child = match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(KindState::Scroll(s)) => s.child,
        _ => None,
    };
    let (Some(child), Some(frame), Some(&rect)) =
        (child, model.arena.frame(h), model.frame_rects.get(&h))
    else {
        return [0.0, 0.0];
    };
    let mut content = SpanBox {
        left: f32::MAX,
        bottom: f32::MAX,
        right: 0.0,
        top: 0.0,
    };
    join_subtree(model, child, &mut content);
    let floor = |x: f64| if x > 0.0 { x } else { 0.0 };
    let inv = 1.0 / f64::from(frame.effective_scale);
    let x =
        floor(f64::from(content.right) - f64::from(content.left) - f64::from(rect.width())) * inv;
    let y = floor(f64::from(content.top) - f64::from(content.bottom) - f64::from(rect.height()))
        * f64::from(inv as f32);
    [x as f32, y as f32]
}

/// The content box `0x786f80` grows, in the layout's units.
struct SpanBox {
    left: f32,
    bottom: f32,
    right: f32,
    top: f32,
}

impl SpanBox {
    /// Grow to cover `r`; a NaN edge leaves the box's (the unordered compares keep it).
    fn join(&mut self, r: &crate::layout::Rect) {
        self.left = self.left.min(r.left);
        self.bottom = self.bottom.min(r.bottom);
        self.right = self.right.max(r.right);
        self.top = self.top.max(r.top);
    }
}

/// `0x786f80`: join `f`'s own resolved rect, shown or not (`0x786fb0`), each shown region's
/// (`0x78703b`), and recursively each child frame whose own shown bit is set (`+0xd0`,
/// `0x7870e0`), so a shown descendant of a hidden scroll child still counts. A part with no
/// resolved rect adds nothing.
fn join_subtree(model: &Model, f: FrameHandle, content: &mut SpanBox) {
    let Some(frame) = model.arena.frame(f) else {
        return;
    };
    if let Some(r) = model.frame_rects.get(&f) {
        content.join(r);
    }
    for &rh in &frame.regions {
        if model.region_data.get(&rh).is_some_and(|d| d.hidden) {
            continue;
        }
        if let Some(r) = model.region_rects.get(&rh) {
            content.join(r);
        }
    }
    for &ch in &frame.children {
        if model.arena.frame(ch).is_some_and(|c| c.shown) {
            join_subtree(model, ch, content);
        }
    }
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
            with_scroll(lua, &this, |s| s.range_dirty = true)?;
            {
                let h = frame_handle_of(lua, &this)?;
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model.arena.set_scroll_child(h, new_child);
            }
            // The old child keeps its point and is not invalidated (`0x786ce6`–`0x786cf4` only
            // flags it); the new one is re-anchored on every call (`0x786d11`).
            reanchor_child(lua, &this)?;
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

    // SetVerticalScroll(px) (`0x791400`): `lua_isnumber` (`0x791490`) else
    // `Usage: %s:SetVerticalScroll(offset)` (`0x87b59c`) naming the frame. Then `0x786db0` stores
    // the offset unclamped (`[+0x328]`), re-anchors the child (`0x787100`) and fires
    // `OnVerticalScroll(self, offset)`, all only on a change: one under 2⁻²² layout units between
    // the offsets as stored (`S`, `0x7914a8`; `0x786dc2`) is dropped whole, and a NaN, unordered,
    // always stores and fires (`0x786dcd`). The range is never read here: every clamp
    // is FrameXML's, through the scroll bar's min and max.
    m.set(
        "SetVerticalScroll",
        lua.create_function(|lua, (this, px): (Table, Value)| {
            with_scroll(lua, &this, |_| ())?;
            let Some(px) = lua.coerce_number(px)? else {
                return Err(binding_abi::usage(lua, &this, "SetVerticalScroll(offset)"));
            };
            let px = px as f32;
            let units = script_units(lua);
            let changed = with_scroll(lua, &this, |s| {
                let changed = moved(units.store(s.vertical), units.store(px), SIZE_EPS);
                if changed {
                    s.vertical = px;
                }
                changed
            })?;
            if !changed {
                return Ok(());
            }
            // The re-anchor at the new offset (`0x786dd8` → `0x787100`).
            reanchor_child(lua, &this)?;
            fire_scroll(lua, &this, "OnVerticalScroll", units.load(units.store(px)))
        })?,
    )?;
    m.set(
        "GetVerticalScroll",
        lua.create_function(|lua, this: Table| {
            let v = with_scroll(lua, &this, |s| s.vertical)?;
            let units = script_units(lua);
            Ok(units.load(units.store(v)))
        })?,
    )?;
    // The stored range (`0x7917d0` reads `[+0x320]` at `0x791873`) through the getter steps,
    // stale while the frame is hidden or its range unmarked.
    m.set(
        "GetVerticalScrollRange",
        lua.create_function(|lua, this: Table| {
            let range = with_scroll(lua, &this, |s| s.range[1])?;
            Ok(script_units(lua).load(range))
        })?,
    )?;

    // ── The horizontal axis ──
    // SetHorizontalScroll(px) (`0x7912f0` → `0x786d30`, `Usage: %s:SetHorizontalScroll(offset)`
    // `0x87b574`) is the vertical setter's byte-clone on `[+0x324]`, firing `OnHorizontalScroll`.
    // The sign is the reference's: `0x787100` anchors the child at `(+hScroll, +vScroll)`, so a
    // positive x moves it right, where a positive y lifts it.
    m.set(
        "SetHorizontalScroll",
        lua.create_function(|lua, (this, px): (Table, Value)| {
            with_scroll(lua, &this, |_| ())?;
            let Some(px) = lua.coerce_number(px)? else {
                return Err(binding_abi::usage(
                    lua,
                    &this,
                    "SetHorizontalScroll(offset)",
                ));
            };
            let px = px as f32;
            let units = script_units(lua);
            let changed = with_scroll(lua, &this, |s| {
                let changed = moved(units.store(s.horizontal), units.store(px), SIZE_EPS);
                if changed {
                    s.horizontal = px;
                }
                changed
            })?;
            if !changed {
                return Ok(());
            }
            reanchor_child(lua, &this)?;
            fire_scroll(
                lua,
                &this,
                "OnHorizontalScroll",
                units.load(units.store(px)),
            )
        })?,
    )?;
    m.set(
        "GetHorizontalScroll",
        lua.create_function(|lua, this: Table| {
            let v = with_scroll(lua, &this, |s| s.horizontal)?;
            let units = script_units(lua);
            Ok(units.load(units.store(v)))
        })?,
    )?;
    m.set(
        "GetHorizontalScrollRange",
        lua.create_function(|lua, this: Table| {
            let range = with_scroll(lua, &this, |s| s.range[0])?;
            Ok(script_units(lua).load(range))
        })?,
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

/// The screen-scale cells the range getters convert through (`0x7916f0`, `0x7917d0`).
fn script_units(lua: &Lua) -> crate::layout::LayoutUnits {
    lua.app_data_ref::<Model>()
        .expect("model app_data")
        .layout_units()
}

/// Whether `new` is a change from `old` by the reference's `fabs; fcomp [0x8029d4]; test ah,5`
/// (`0x786dc2`, `0x786ede`): at least `eps` apart, or unordered, so a NaN is always a change.
fn moved(old: f32, new: f32, eps: f64) -> bool {
    let d = (f64::from(new) - f64::from(old)).abs();
    d >= eps || d.is_nan()
}

/// Fire `On{Vertical,Horizontal}Scroll(self, offset)`, the reference's script slots `[+0x334]` and
/// `[+0x32c]`, outside any model borrow. The offset is the stored one through the getter steps,
/// `G([+0x328])` pushed as a double under `"%f"` (`0x786ded`–`0x786e10`, `0x835160`).
fn fire_scroll(lua: &Lua, this: &Table, script: &str, value: f64) -> mlua::Result<()> {
    let id = {
        let h = frame_handle_of(lua, this)?;
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        model.frame_id(h)
    };
    if let Err(e) = event::fire_widget_handler(lua, id, script, vec![Value::Number(value)]) {
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
/// `OnScrollRangeChanged(self, xRange, yRange)` when either moved by 2⁻²² (`[0x8029d4]`) layout
/// units or more (`0x786ed7`–`0x786efd`), each range through the getter steps.
///
/// The rects it reads are the ones the pump's drain before the walk (`0x7656ba`) resolved: the
/// recompute reads them through `0x768320`, which never resolves, so a resize made earlier in the
/// walk counts from the next tick, and the size it notes is the one that drain applied.
pub(super) fn update(lua: &Lua, h: FrameHandle) {
    let notify = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let rect = model.frame_rects.get(&h).copied();
        let eps = SIZE_EPS;
        let units = model.layout_units();
        let Some(s) = scroll_state(&mut model, h) else {
            return;
        };
        see_size(s, rect, eps);
        if !s.range_dirty {
            return;
        }
        let has_child = s.child.is_some();
        let measured = (rect.is_some() && has_child).then(|| scroll_ranges(&model, h));
        let Some(s) = scroll_state(&mut model, h) else {
            return;
        };
        let moved = measured.filter(|new| {
            let old = std::mem::replace(&mut s.range, *new);
            // Unless both moved under 2^-22 (`0x786ede`, `0x786ef2`): a NaN fires.
            (0..2).any(|i| moved(old[i], new[i], eps))
        });
        if moved.is_none() {
            s.range_dirty = false;
        }
        moved.map(|range| (model.frame_id(h), range.map(|r| units.load(r))))
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
        vec![Value::Number(x_range), Value::Number(y_range)],
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

/// Note the frame's resolved size in layout units; a change since the last note marks the range, by
/// the width or height move that fires `OnSizeChanged` ([`crate::layout::size_changed`] at `eps`,
/// [`crate::layout::SIZE_EPS`]). No rect, no note.
fn see_size(s: &mut ScrollFrameState, rect: Option<crate::layout::Rect>, eps: f64) {
    let Some(r) = rect else { return };
    let size = [r.width(), r.height()];
    let moved = |i: usize| (f64::from(size[i]) - f64::from(s.seen_size[i])).abs() >= eps;
    if moved(0) || moved(1) {
        s.range_dirty = true;
    }
    s.seen_size = size;
}

/// The re-anchor (`0x787100`): when the frame holds a child, clear its points (`0x787110` →
/// `0x767620`) and set one TOPLEFT on the frame's TOPLEFT at the raw `(horizontal, vertical)`
/// offsets, y up, with the invalidate flag (`0x787136`). A real write on the child: its own
/// points are gone, and it keeps the new one when the frame lets it go.
fn reanchor_child(lua: &Lua, this: &Table) -> mlua::Result<()> {
    let (child, x, y) = with_scroll(lua, this, |s| (s.child, s.horizontal, s.vertical))?;
    let Some(child) = child else {
        return Ok(());
    };
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model");
    if model.arena.frame(child).is_none() {
        return Ok(());
    }
    let sid = model.frame_id(h);
    let pin = crate::layout::Anchors::from([crate::layout::Anchor::new(
        crate::layout::Point::TopLeft,
        sid,
        crate::layout::Point::TopLeft,
        x,
        y,
    )]);
    let input = model.layout_inputs.entry(child).or_default();
    let changed = !input.anchors.bits_eq(&pin);
    input.anchors = pin;
    model.invalidate_frame(child, changed);
    Ok(())
}
