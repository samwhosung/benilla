//! Frame methods: anchors, size, and the resolved geometry readers.

use mlua::{Lua, MultiValue, Table, Value};

use crate::layout::{Anchor, Point};
use crate::script::region_map::{set_shared, Side};
use crate::script::{Model, SCREEN};
use crate::widget::FrameHandle;

use super::anchor_args::{parse_set_all_points, parse_set_point, resolve_rel_target, UNNAMED};
use super::{frame_handle_of, frame_parent_token_base, frame_wrapper, point_name};

/// The receiver's name for the error strings and its `$parent` base, read under one short `Model`
/// borrow that ends before the ladder's `_G` read.
fn frame_ladder_context(lua: &Lua, h: FrameHandle) -> (String, String) {
    let model = lua.app_data_ref::<Model>().expect("model");
    let who = model
        .arena
        .frame(h)
        .and_then(|f| f.name.clone())
        .unwrap_or_else(|| UNNAMED.to_string());
    (who, frame_parent_token_base(&model, h))
}

/// Populate `m`'s anchor and size methods.
pub(super) fn install(lua: &Lua, m: &Table) -> mlua::Result<()> {
    // Layout: SetPoint / ClearAllPoints / SetWidth / SetHeight / GetWidth / GetHeight
    set_shared(
        lua,
        m,
        Side::Frame,
        "SetPoint",
        |lua, (this, rest): (Table, MultiValue)| set_point(lua, &this, &rest),
    )?;
    set_shared(lua, m, Side::Frame, "ClearAllPoints", |lua, this: Table| {
        let h = frame_handle_of(lua, &this)?;
        let mut model = lua.app_data_mut::<Model>().expect("model");
        // Every layout setter mutates only on a real change, so a per-frame caller setting the
        // same value never reopens the layout gate. Clearing is a retarget to no targets.
        let old: Option<Vec<u32>> = match model.layout_inputs.get_mut(&h) {
            Some(input) if !input.anchors.is_empty() => {
                let old = input.anchors.iter().map(|a| a.relative_to).collect();
                input.anchors.clear();
                Some(old)
            }
            _ => None,
        };
        if let Some(old) = old {
            model.touch_layout_retarget_frame(h, &old, &[]);
        }
        Ok(())
    })?;
    // GetPoint([n]) → point, relativeTo, relativePoint, x, y of the n-th anchor (default 1);
    // relativeTo is nil for a screen-root anchor, since `SCREEN` has no wrapper.
    set_shared(
        lua,
        m,
        Side::Frame,
        "GetPoint",
        |lua, (this, n): (Table, Option<i64>)| {
            let h = frame_handle_of(lua, &this)?;
            let anchor = {
                let model = lua.app_data_ref::<Model>().expect("model");
                let idx = (n.unwrap_or(1).max(1) - 1) as usize;
                model
                    .layout_inputs
                    .get(&h)
                    .and_then(|i| i.anchors.get(idx))
                    .cloned()
            };
            let Some(a) = anchor else {
                return Ok((Value::Nil, Value::Nil, Value::Nil, Value::Nil, Value::Nil));
            };
            let rel = if a.relative_to == SCREEN {
                Value::Nil
            } else {
                Value::Table(frame_wrapper(lua, a.relative_to)?)
            };
            Ok((
                Value::String(lua.create_string(point_name(a.point))?),
                rel,
                Value::String(lua.create_string(point_name(a.relative_point))?),
                Value::Number(f64::from(a.x_off)),
                Value::Number(f64::from(a.y_off)),
            ))
        },
    )?;
    // GetNumPoints(): on the Region method table (`0x87c9b8`), so every widget answers it.
    set_shared(lua, m, Side::Frame, "GetNumPoints", |lua, this: Table| {
        let h = frame_handle_of(lua, &this)?;
        let model = lua.app_data_ref::<Model>().expect("model");
        Ok(model
            .layout_inputs
            .get(&h)
            .map_or(0, |i| i.anchors.len() as i64))
    })?;
    // SetAllPoints([relativeTo]): pins TOPLEFT and BOTTOMRIGHT to the target, default the parent,
    // as XML `setAllPoints="true"` does (`0x767800`).
    set_shared(
        lua,
        m,
        Side::Frame,
        "SetAllPoints",
        |lua, (this, rest): (Table, MultiValue)| {
            let h = frame_handle_of(lua, &this)?;
            // `who` and `$parent`, then the `_G` read, then the model guard, as in `set_point`.
            let (who, base) = frame_ladder_context(lua, h);
            let target = parse_set_all_points(lua, rest.front(), &base);
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let me = model.frame_id(h);
            let parent = default_parent_id(&mut model, h);
            let rel_id: u32 =
                resolve_rel_target(&model, &target, &who, "SetAllPoints", me, parent)?;
            let pair = [
                Anchor::new(Point::TopLeft, rel_id, Point::TopLeft, 0.0, 0.0),
                Anchor::new(Point::BottomRight, rel_id, Point::BottomRight, 0.0, 0.0),
            ];
            let input = model.layout_inputs.entry(h).or_default();
            let same = input.anchors.len() == 2
                && input
                    .anchors
                    .iter()
                    .zip(&pair)
                    .all(|(a, b)| anchor_bits_eq(a, b));
            if !same {
                input.anchors.clear();
                input.anchors.extend_from_slice(&pair);
                model.touch_layout();
            }
            Ok(())
        },
    )?;
    set_shared(
        lua,
        m,
        Side::Frame,
        "SetWidth",
        |lua, (this, w): (Table, f32)| {
            let h = frame_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let input = model.layout_inputs.entry(h).or_default();
            let changed = input.width.to_bits() != w.to_bits();
            input.width = w;
            if changed {
                // A size write moves no edge and no roster membership.
                model.touch_layout_frame(h);
            }
            model.note_authored_size(h);
            Ok(())
        },
    )?;
    set_shared(
        lua,
        m,
        Side::Frame,
        "SetHeight",
        |lua, (this, ht): (Table, f32)| {
            let h = frame_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let input = model.layout_inputs.entry(h).or_default();
            let changed = input.height.to_bits() != ht.to_bits();
            input.height = ht;
            if changed {
                model.touch_layout_frame(h);
            }
            model.note_authored_size(h);
            Ok(())
        },
    )?;
    // No `SetSize`: neither 1.12 method table has it.
    set_shared(lua, m, Side::Frame, "GetWidth", |lua, this: Table| {
        let h = frame_handle_of(lua, &this)?;
        settle(lua);
        let model = lua.app_data_ref::<Model>().expect("model");
        Ok(size_read(&model, h, true))
    })?;
    set_shared(lua, m, Side::Frame, "GetHeight", |lua, this: Table| {
        let h = frame_handle_of(lua, &this)?;
        settle(lua);
        let model = lua.app_data_ref::<Model>().expect("model");
        Ok(size_read(&model, h, false))
    })?;

    // GetCenter(): the resolved rect's centre in the frame's own units (screen ÷ effective scale,
    // y-up), as the reference's coordinate getters report; nil before the first resolve.
    set_shared(lua, m, Side::Frame, "GetCenter", |lua, this: Table| {
        let h = frame_handle_of(lua, &this)?;
        settle(lua);
        let model = lua.app_data_ref::<Model>().expect("model");
        let inv = 1.0 / eff_scale(&model, h);
        Ok(match model.resolved.get(&h) {
            Some(r) => (
                Value::Number(f64::from((r.left + r.right) * 0.5 * inv)),
                Value::Number(f64::from((r.bottom + r.top) * 0.5 * inv)),
            ),
            None => (Value::Nil, Value::Nil),
        })
    })?;

    // GetEffectiveScale(): parent scale times own scale, from a root of 1: `uiScale` is applied at
    // the raster seam (a screen `768/uiScale` units tall), so every coordinate Lua sees,
    // `GetCursorPosition()` included, is already in UI units. The reference instead makes
    // `uiScale` `UIParent`'s own scale (`0x494550` calls `SetScale`), so there
    // `UIParent:GetEffectiveScale()` answers it.
    m.set(
        "GetEffectiveScale",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(f64::from(eff_scale(&model, h)))
        })?,
    )?;

    // GetLeft/GetRight/GetTop/GetBottom: the resolved edges in the frame's own units, y-up from
    // the screen bottom (`GetRect 0x768320`); nil before the first resolve.
    for (name, pick) in [
        ("GetLeft", 0u8),
        ("GetRight", 1u8),
        ("GetTop", 2u8),
        ("GetBottom", 3u8),
    ] {
        set_shared(lua, m, Side::Frame, name, move |lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            settle(lua);
            let model = lua.app_data_mut::<Model>().expect("model");
            let inv = 1.0 / eff_scale(&model, h);
            Ok(model.resolved.get(&h).map(|r| {
                inv * match pick {
                    0 => r.left,
                    1 => r.right,
                    2 => r.top,
                    _ => r.bottom,
                }
            }))
        })?;
    }
    Ok(())
}

/// Resolve the layout now if anything moved, as the reference answers a geometry query against
/// current layout; a settled tree costs one epoch compare. Deviation: `OnSizeChanged` fires at the
/// next `UiScript::resolve`, a tick after the reference's (`ApplyRect 0x76b580`), because running
/// Lua handlers inside a binding risks a borrow panic or unbounded recursion.
fn settle(lua: &Lua) {
    let mut model = lua.app_data_mut::<Model>().expect("model");
    crate::script::UiScript::resolve_layout(&mut model);
}

pub(crate) fn eff_scale(model: &Model, h: FrameHandle) -> f32 {
    let s = model
        .arena
        .frame(h)
        .map(|f| f.effective_scale)
        .unwrap_or(1.0);
    if s.abs() < 1e-6 {
        1.0
    } else {
        s
    }
}

/// `GetWidth`/`GetHeight`: the resolved span in the frame's own units (screen ÷ effective scale),
/// else the size it was given, where 0 means derived, as in the reference.
fn size_read(model: &Model, h: FrameHandle, width: bool) -> f32 {
    if let Some(r) = model.resolved.get(&h) {
        let span = if width { r.width() } else { r.height() };
        return span / eff_scale(model, h);
    }
    model
        .layout_inputs
        .get(&h)
        .map(|i| if width { i.width } else { i.height })
        .unwrap_or(0.0)
}

/// `SetPoint` (`0x7a2540`) for a frame: the argument ladder is [`super::anchor_args`], shared with
/// regions; this side supplies the default parent and commits the anchor.
fn set_point(lua: &Lua, this: &Table, args: &MultiValue) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let (who, base) = frame_ladder_context(lua, h);
    // No model guard is alive across the ladder: its `_G` read is a `lua_gettable`, and an
    // `__index` can call back in.
    let p = parse_set_point(lua, args, &who, &base)?;

    let mut model = lua.app_data_mut::<Model>().expect("model");
    let me = model.frame_id(h);
    let parent = default_parent_id(&mut model, h);
    let rel_to_id = resolve_rel_target(&model, &p.target, &who, "SetPoint", me, parent)?;
    let (point, rel_point, x, y) = (p.point, p.rel_point, p.x, p.y);

    let input = model.layout_inputs.entry(h).or_default();
    let new = Anchor::new(point, rel_to_id, rel_point, x, y);
    // A no-op only when the identical anchor is already last and no earlier one has this point,
    // mirroring the retain and push below; bits compared, as the fingerprint tells -0.0 from 0.0.
    let same_at_tail = input
        .anchors
        .last()
        .is_some_and(|a| anchor_bits_eq(a, &new))
        && !input.anchors[..input.anchors.len() - 1]
            .iter()
            .any(|a| a.point == point);
    if !same_at_tail {
        // Target lists are collected only for a retarget: the value-only change is the per-frame
        // idiom (a dragged window) and must stay allocation-free.
        let structural = anchor_retarget_is_structural(&input.anchors, &new);
        let old_targets: Option<Vec<u32>> =
            structural.then(|| input.anchors.iter().map(|a| a.relative_to).collect());
        input.anchors.retain(|a| a.point != point);
        input.anchors.push(new);
        match old_targets {
            None => model.touch_layout_frame(h),
            Some(old) => {
                let new_targets: Vec<u32> = model.layout_inputs[&h]
                    .anchors
                    .iter()
                    .map(|a| a.relative_to)
                    .collect();
                model.touch_layout_retarget_frame(h, &old, &new_targets);
            }
        }
    }
    Ok(())
}

/// Whether this `SetPoint` changes the node's anchor targets, mirroring the setters' retain and
/// push: value-only exactly when one existing anchor has this point and keeps its target.
pub(crate) fn anchor_retarget_is_structural(anchors: &[Anchor], new: &Anchor) -> bool {
    let mut same_point = anchors.iter().filter(|a| a.point == new.point);
    match (same_point.next(), same_point.next()) {
        (Some(old), None) => old.relative_to != new.relative_to,
        _ => true,
    }
}

/// Bit-exact anchor equality, as the layout gate's fingerprint compares, so the two always agree.
pub(crate) fn anchor_bits_eq(a: &Anchor, b: &Anchor) -> bool {
    a.point == b.point
        && a.relative_to == b.relative_to
        && a.relative_point == b.relative_point
        && a.x_off.to_bits() == b.x_off.to_bits()
        && a.y_off.to_bits() == b.y_off.to_bits()
}

/// The default `relativeTo`: the parent, or [`SCREEN`] for a top-level frame (`0x76c6e0`).
fn default_parent_id(model: &mut Model, h: FrameHandle) -> u32 {
    match model.arena.frame(h).and_then(|f| f.parent) {
        Some(p) => model.frame_id(p),
        None => SCREEN,
    }
}
