//! The `ScrollFrame` method surface (`CSimpleScrollFrame`) — the ScrollFrame mechanism (decision
//! 0112, the engine's last structural gap): `SetScrollChild`/`GetScrollChild`, the two scroll
//! offsets (`Set`/`GetVerticalScroll`, `Set`/`GetHorizontalScroll`), and the live ranges
//! (`Get{Vertical,Horizontal}ScrollRange`/`UpdateScrollChildRect`). The offset setters and the
//! ranges are byte-pinned (decisions 1338, 2017): `0x786db0` stores the offset VERBATIM — the
//! engine never clamps it — and `0x786e30` measures both ranges off the scroll child's subtree. The
//! rest is spec-faithful to the documented widget contract (the Era `ScrollFrameTemplate` Lua
//! drives its scrollbar off exactly these), same posture as StatusBar's fill.
//!
//! **The two axes are one mechanism, not a mechanism and a special case.** The reference's nine
//! ScrollFrame bindings are four symmetric pairs plus `UpdateScrollChildRect`, and the C++ under
//! them is literally cloned: `0x786d30` (horizontal) is the byte-clone of `0x786db0` (vertical)
//! against `[+0x324]`/`[+0x32c]` instead of `[+0x328]`/`[+0x334]`, and one `0x786e30` computes
//! both ranges out of one subtree union. This module
//! is written that way too — one helper per concept, taken on the axis asked for. That the
//! horizontal half went unwritten until the corpus asked for it (`aux-addon` reads and writes both
//! axes in one function) is why this doc used to say horizontal scroll "is out of scope: no 1.12
//! template drives it" — true of the stock templates, and never true of the addons.
//!
//! The actual geometry — the scroll child's anchors pinned to the frame + the scroll offsets, and
//! the clip every descendant of the child draws/hits within — lives in [`super::mod@super`]'s
//! `resolve`/`extract`/`hit_test` (the layout-graph override + the ancestor-walk clip helper); this
//! module is the thin Lua binding over the per-frame [`ScrollFrameState`], plus the three script
//! hooks (`OnVerticalScroll`/`OnHorizontalScroll`/`OnScrollRangeChanged`) a scrollbar's
//! `OnValueChanged`/`OnLoad` wires to.
//!
//! The methods live in their own registry table, consulted by the frame `__index` dispatcher only
//! for ScrollFrame frames — so duck-typing addons (`if frame.SetScrollChild then …`) see `nil` on
//! every other kind, exactly as against the client's per-class method sets.

use mlua::{Lua, Table, Value};

use super::object::{decode_id, frame_handle_of, frame_wrapper};
use super::{event, Model};
use crate::widget::{FrameHandle, KindState, ScrollFrameState};

/// Registry key of the ScrollFrame method table (the MAXCSTACK discipline: Lua-side root, named key).
pub(super) const REG_SCROLLFRAME_METHODS: &str = "__benilla_scrollframe_methods";

/// Run `f` over a frame's ScrollFrame state under one short write borrow. Errors if `this` is not a
/// live ScrollFrame (unreachable through the kind dispatcher, but the method table is a plain Lua
/// value — a caller could fish it out and misapply it).
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

/// Which axis a scroll offset, span or range is taken on.
///
/// One type rather than two code paths, because the reference computes both ranges in ONE pass
/// over ONE subtree union (`0x786e30`: `[+0x31c] = max(0, maxRIGHT − minLEFT − width) / scale`,
/// `[+0x320] = max(0, maxTOP − minBOTTOM − height) / scale`) — the axes differ only in which pair
/// of edges the union tracks and which frame dimension is subtracted.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Axis {
    /// `maxRIGHT − minLEFT` against the frame's width — `GetHorizontalScrollRange`, `[+0x31c]`.
    Horizontal,
    /// `maxTOP − minBOTTOM` against the frame's height — `GetVerticalScrollRange`, `[+0x320]`.
    Vertical,
}

/// `Get{Vertical,Horizontal}ScrollRange()`'s live computation: `max(0, contentExtent − frameExtent)`
/// from the **resolved** rects — `0.0` when the frame is unresolved or there is no child. Never
/// cached: the scroll offset always clamps against the current layout, not a stale snapshot.
///
/// **The content extent is the scroll child's whole SUBTREE, not the child frame's own height**
/// (decision 1338 corrects what 0112 shipped). `0x786e30` seeds a bounding box
/// `{FLT_MAX, FLT_MAX, 0, 0}` and calls the **recursive** `0x786f80(scrollChild, &bbox)`, which
/// unions the frame's REGION list (head `+0x1b4`, link `+0x1b8`, guard `[entry+8]+0xc4`) and
/// re-enters itself for every CHILD FRAME (head `+0x2fc`, link `+0x300`, guard `[entry+8]+0xd0`);
/// the range stored at `[+0x31c]` is `max(0, bboxExtent − viewportExtent)`.
///
/// The distinction is not academic: stock `ItemTextPageScrollChild` is declared **10×10**
/// (`ItemTextFrame.xml` l.198) around a 270×304 SimpleHTML whose blocks extend well past it.
/// Measured as the child's own height the range is zero and a book cannot be scrolled at all;
/// measured as the subtree box it tracks the rendered text. Every ScrollFrame whose child is sized
/// directly — the guild-info EditBox, the options panes, every faux list — is unchanged, because
/// there its own rect already IS its subtree.
///
/// In **LOCAL units**, each extent divided by the owning frame's effective scale — i.e. exactly
/// `child:GetHeight() - self:GetHeight()`, the widget contract this method states. The resolved
/// rects are screen px, and the scroll offset is NOT: `SetVerticalScroll` becomes the scroll
/// child's anchor y-offset, which the solver multiplies by that child's scale (`resolve`'s
/// override; `anchor_resolve_y`). Reporting a screen-px range against a local-unit offset makes
/// every SCALED ScrollFrame under-scroll by exactly its scale — the bar's max is the range, so it
/// stops short of the end and the tail of the content is unreachable. Invisible at scale 1.0,
/// which is every 1.12-native window; the era-scaled options window (ERA_WINDOW_SCALE 0.78) is the
/// first real-scroll customer that isn't, and it lost 22% of its travel.
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

/// The span of `f`'s subtree along `axis`, in SCREEN px, as `(far, near)` — `(top, bottom)`
/// vertically, `(right, left)` horizontally — its own resolved rect unioned with every VISIBLE
/// descendant region and child frame, recursively (`0x786f80`). `None` when nothing in the subtree
/// resolved.
///
/// Visibility is the client's own guard on both lists: a hidden block contributes no scroll range,
/// which is what stops a window's parked art from inventing travel nobody can use.
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

    // SetScrollChild(frame|name|nil) — a wrapper table or a name string resolves like every other
    // frame-target arg (SetParent, SetPoint's relativeTo); nil (or an unresolvable target) clears.
    m.set(
        "SetScrollChild",
        lua.create_function(|lua, (this, target): (Table, Value)| {
            // `_G[name]` before the guard, no `$parent` — `0x790fa0`'s string arm calls `0x76c760`
            // directly, the same narrow-Frame-tag resolver `SetParent` uses
            // (`object::NamedTarget`).
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
                // The child override is part of the resolve's read set (decision 0112's local
                // anchor map — a fingerprint input), so a real re-target dirties tier 1.
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

    // SetVerticalScroll(px) — stores the offset VERBATIM and, only when it actually changed,
    // re-lays the child out and fires OnVerticalScroll(self, offset) — the scrollbar's
    // OnValueChanged wiring's other half.
    //
    // **No clamp** (decision 2017). The reference's `0x786db0` compares the new value against the
    // OLD one alone (an epsilon change-gate: unchanged → nothing happens, not even the script),
    // writes it to `[+0x328]`, re-anchors the scroll child off it (`0x787100` →
    // `SetPoint(child, self, hScroll, vScroll)`) and fires the handler. `[+0x320]` — the range —
    // is never read on this path. Every clamp the reference exhibits is FrameXML's own, through
    // the scroll bar's `[min, max]` (`ScrollFrameTemplate_OnMouseWheel`,
    // `ScrollFrame_OnScrollRangeChanged`). The clamp this carried from 07-05 (a3fce6a76) to 2017
    // cost every faux list whose frame is taller than `rows × step` its last rows (B370): the
    // reference's `FauxScrollFrame_Update` drives the bar to `(n − rows) × step`, which is past
    // `n × step − frameHeight` whenever the frame has slack under its last row.
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

    // ── The horizontal axis: the same three verbs on `[+0x324]`/`[+0x31c]` ─────────────────────
    //
    // `SetHorizontalScroll(px)` `0x7912f0` → `0x786d30`, which is the BYTE-CLONE of
    // `SetVerticalScroll`'s `0x786db0` against the horizontal fields.
    // So every law stated above holds verbatim on this axis: no clamp, the same 2⁻²² change-gate
    // against the OLD offset alone, the same re-anchor, and `OnHorizontalScroll(self, offset)`
    // fired only on a real change. Nothing here is a horizontal special case.
    //
    // The one thing that IS specific to this axis is the SIGN, and it is the reference's:
    // `0x787100` passes both offsets to `SetPoint(TOPLEFT, self, TOPLEFT, +hScroll, +vScroll)`
    // unnegated, so positive x pushes the child right (revealing nothing) where positive y lifts it
    // (revealing content below). `aux-addon/tabs/search/filter.lua:315-322` — the corpus caller
    // that made this pair worth writing, reading both axes in one line and writing both in the next
    // — bounds x into `[min(0, frameWidth - contentWidth - 10), 0]` and y into
    // `[0, contentHeight - frameHeight]`, which is the same asymmetry read off the other side of
    // the API. See [`ScrollFrameState::horizontal`].
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
    // UpdateScrollChildRect() — the Era template calls this after resizing its content to re-drive
    // its scrollbar. Our range is always live (never cached) against the RESOLVED rects — but a
    // script resize (`child:SetHeight(...)`) lands in `LayoutInput` immediately, while `resolved`
    // only catches up on the app's own next `resolve()` call (Lua runs before that each tick,
    // `ui_script/mod.rs`'s drive loop). This is precisely the Era idiom this method exists to
    // serve — resize the content, then immediately ask for the new range — so a stale `resolved`
    // would make its very own primary caller see a one-tick-old (often wrong-by-a-lot, e.g. still
    // "nothing to scroll") answer. Force a fresh resolve first: cheap on a quiet graph (the
    // fixpoint's own doc — an unrelated resize elsewhere converges in one round), and correctness
    // here matters more than dodging a resolve pass a caller who wants nothing stale already asked
    // for by name.
    m.set(
        "UpdateScrollChildRect",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            // The reference derives the range from a child whose text is already laid out — its
            // measure is synchronous. Ours measures on the metric reads and on the host's
            // round-trip, which lands a frame later; stock `QuestLog_UpdateQuestDetails` calls
            // this right after the SetTexts that fill the pane, and read a range of 0 (1944). So
            // the child's FontStrings are measured here, through the installed measurer, before
            // the layout that the range reads; with no measurer installed the round-trip fills
            // them as before.
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
            // That forced resolve can have MOVED sizes — this method exists precisely because the
            // caller just resized its content — so drain the queue before the range notify: the
            // reference fires `OnSizeChanged` from `ApplyRect`, i.e. during the layout, not after.
            event::fire_size_changes(lua);
            // `OnScrollRangeChanged(self, xRange, yRange)` — **horizontal first**, and that order is
            // byte-derived rather than assumed: at `0x786eff` the vertical result is on top of the
            // x87 stack, is `fstp`'d first and therefore pushed DEEPEST, so it lands as `arg2`. It
            // is why stock `UIPanelScrollFrameTemplate` hands `arg2` to
            // `ScrollFrame_OnScrollRangeChanged`.
            // `arg1` was a hardcoded `0.0` here for as long as there was no horizontal range to
            // put in it.
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

/// Fire `On{Vertical,Horizontal}Scroll(self, offset)` (the ScrollFrame's own script slots — the
/// reference's `[+0x334]`/`[+0x32c]`, fired from the setter clones `0x786db0`/`0x786d30`). Fired
/// outside any model borrow; errors go to [`Model::errors`] like every other widget handler.
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
