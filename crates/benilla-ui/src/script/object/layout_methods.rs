//! Frame methods: anchors, size, and the resolved geometry readers.

use mlua::{Lua, MultiValue, Table, Value};

use crate::layout::{Anchor, Anchors, Point};
use crate::script::region_map::{set_shared, Side};
use crate::script::{Model, SCREEN};
use crate::widget::FrameHandle;

use super::anchor_args::{
    get_point_index, get_point_values, parse_set_all_points, parse_set_point, resolve_rel_target,
    UNNAMED,
};
use super::{frame_handle_of, frame_parent_token_base, frame_wrapper};

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
        // `0x767620` frees the anchors and unhooks each from its target, and queues no resolve:
        // the frame keeps its rect until its own next invalidate. Named, as the graph loses the
        // edges. Every layout setter mutates only on a real change, so a per-frame caller setting
        // the same value never reopens the layout gate.
        let cleared = model
            .layout_inputs
            .get_mut(&h)
            .is_some_and(|input| input.anchors.clear());
        if cleared {
            model.touch_layout_frame(h);
        }
        Ok(())
    })?;
    // GetPoint([n]) → point, relativeTo, relativePoint, x, y of the n-th anchor in point order
    // (`0x7a2340`); relativeTo is nil for a screen-root anchor, since `SCREEN` has no wrapper.
    set_shared(
        lua,
        m,
        Side::Frame,
        "GetPoint",
        |lua, (this, n): (Table, Value)| {
            let h = frame_handle_of(lua, &this)?;
            let idx = get_point_index(lua, n)?;
            let (anchor, units) = {
                let model = lua.app_data_ref::<Model>().expect("model");
                let anchor = model
                    .layout_inputs
                    .get(&h)
                    .and_then(|i| i.anchors.nth(idx))
                    .copied();
                (anchor, model.layout_units())
            };
            get_point_values(lua, anchor, units, |rel| {
                Ok(if rel == SCREEN {
                    Value::Nil
                } else {
                    Value::Table(frame_wrapper(lua, rel)?)
                })
            })
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
            // The reference clears, sets both and invalidates (`0x767db0`); a bit-identical pair
            // moves no input.
            let pair = Anchors::from(pair);
            let changed = !input.anchors.bits_eq(&pair);
            input.anchors = pair;
            model.invalidate_frame(h, changed);
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
            // A size write moves no edge and no roster membership. It invalidates whether or not
            // the width moved (`0x7683d0`).
            model.invalidate_frame(h, changed);
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
            model.invalidate_frame(h, changed);
            model.note_authored_size(h);
            Ok(())
        },
    )?;
    // No `SetSize`: neither 1.12 method table has it.
    set_shared(lua, m, Side::Frame, "GetWidth", |lua, this: Table| {
        let h = frame_handle_of(lua, &this)?;
        Ok(size_read(lua, h, true))
    })?;
    set_shared(lua, m, Side::Frame, "GetHeight", |lua, this: Table| {
        let h = frame_handle_of(lua, &this)?;
        Ok(size_read(lua, h, false))
    })?;

    // GetCenter() (`0x7a1810`): the resolved rect's midpoints over the frame's scale, through the
    // getter steps ([`crate::layout::LayoutUnits::center`]); two nils before the first resolve.
    set_shared(lua, m, Side::Frame, "GetCenter", |lua, this: Table| {
        let h = frame_handle_of(lua, &this)?;
        settle_frame(lua, h);
        let model = lua.app_data_ref::<Model>().expect("model");
        let (units, s) = (model.layout_units(), layout_scale(&model, h));
        Ok(match model.frame_rects.get(&h) {
            Some(r) => (
                Value::Number(units.center(r.left, r.right, s)),
                Value::Number(units.center(r.bottom, r.top, s)),
            ),
            None => (Value::Nil, Value::Nil),
        })
    })?;

    // GetEffectiveScale(): parent scale times own scale (`0x76ac90`), from a root of 1. The UI
    // scale is `UIParent`'s own (`0x494550`), so `UIParent:GetEffectiveScale()` answers it and a
    // frame outside `UIParent` does not carry it.
    m.set(
        "GetEffectiveScale",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(f64::from(eff_scale(&model, h)))
        })?,
    )?;

    // GetLeft/GetRight/GetTop/GetBottom (`0x7a1980`, `0x7a1aa0`, `0x7a1bc0`, `0x7a1ce0`): the
    // resolved edge (`GetRect 0x768320`) over the frame's scale through the getter steps
    // ([`crate::layout::LayoutUnits::edge`]), y up from the screen bottom; nil before the first
    // resolve.
    for (name, pick) in [
        ("GetLeft", 0u8),
        ("GetRight", 1u8),
        ("GetTop", 2u8),
        ("GetBottom", 3u8),
    ] {
        set_shared(lua, m, Side::Frame, name, move |lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            settle_frame(lua, h);
            let model = lua.app_data_ref::<Model>().expect("model");
            let (units, s) = (model.layout_units(), layout_scale(&model, h));
            Ok(model.frame_rects.get(&h).map(|r| {
                let e = match pick {
                    0 => r.left,
                    1 => r.right,
                    2 => r.top,
                    _ => r.bottom,
                };
                units.edge(e, s)
            }))
        })?;
    }
    Ok(())
}

/// Resolve the layout now if anything moved, as the reference answers a geometry query against
/// current layout; a settled tree costs one epoch compare. The read frame `h`, if pending, resolves
/// inside the read, its deferral bit left set (`0x7a1a0b` → `0x7680e0(1)`), and its `OnSizeChanged`
/// fires here, as its `ApplyRect` (`0x76b580` → vt+0x48) runs inside the getter.
pub(in crate::script) fn settle_frame(lua: &Lua, h: FrameHandle) {
    let id = lua
        .app_data_ref::<Model>()
        .expect("model")
        .frame_to_id
        .get(&h)
        .copied();
    settle_node(lua, id);
}

/// [`settle_frame`] for a region's rect, through the same getters (`0x7a1980`..).
pub(in crate::script) fn settle_region(lua: &Lua, rh: crate::widget::RegionHandle) {
    let id = lua
        .app_data_ref::<Model>()
        .expect("model")
        .region_to_id
        .get(&rh)
        .copied();
    settle_node(lua, id);
}

fn settle_node(lua: &Lua, id: Option<u32>) {
    let mut model = lua.app_data_mut::<Model>().expect("model");
    match id {
        Some(id) if model.node_pending(id) => {
            model.pull_read(id);
            drop(model);
            crate::script::visibility::resolve_and_fire(lua, vec![id]);
        }
        _ => crate::script::UiScript::resolve_layout(&mut model),
    }
}

/// The frame's layout scale `[+0x7c]`, which the edge and centre getters divide by as it stands
/// (`0x7a1a4e`): no guard, as `SetScale` refuses 0 (`0x768366`).
pub(crate) fn layout_scale(model: &Model, h: FrameHandle) -> f32 {
    model
        .arena
        .frame(h)
        .map(|f| f.effective_scale)
        .unwrap_or(1.0)
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

/// `GetWidth`/`GetHeight` (`0x7a1e00`, `0x7a2030`): the size the frame was given as it is stored
/// (`+0x1c`/`+0x20` → `0x768420`/`0x768410`, `S` of the number set) through the getter steps, with
/// no resolve, whatever its anchors span (`0x7a1e99`–`0x7a1ea5`). Only a stored 0 resolves a
/// pending frame (`0x7a1eb6`) and answers its rect's span, else 0. The span is not divided by the
/// effective scale, though the rect carries it: it is only converted (`0x7a1ee6`, `0x41ae40`),
/// where `GetLeft` divides (`0x7a1a63`).
fn size_read(lua: &Lua, h: FrameHandle, width: bool) -> f64 {
    let (units, stored) = {
        let model = lua.app_data_ref::<Model>().expect("model");
        let units = model.layout_units();
        let set = model
            .layout_inputs
            .get(&h)
            .map_or(0.0, |i| if width { i.width } else { i.height });
        (units, units.store(set))
    };
    if stored != 0.0 {
        return units.load(stored);
    }
    settle_frame(lua, h);
    let model = lua.app_data_ref::<Model>().expect("model");
    model.frame_rects.get(&h).map_or(0.0, |r| {
        if width {
            units.span(r.left, r.right)
        } else {
            units.span(r.bottom, r.top)
        }
    })
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

    let units = model.layout_units();
    let input = model.layout_inputs.entry(h).or_default();
    // An unchanged anchor stores nothing and queues no resolve (`0x767cde` → `0x767d5e`).
    if input
        .anchors
        .set(Anchor::new(point, rel_to_id, rel_point, x, y), units)
    {
        model.invalidate_frame(h, true);
    }
    Ok(())
}

/// The default `relativeTo`: the parent, or [`SCREEN`] for a top-level frame (`0x76c6e0`).
fn default_parent_id(model: &mut Model, h: FrameHandle) -> u32 {
    match model.arena.frame(h).and_then(|f| f.parent) {
        Some(p) => model.frame_id(p),
        None => SCREEN,
    }
}
