//! The Region layout methods: size, anchors and the resolved-rect readers.

use mlua::{Lua, MultiValue, Table, Value};

use crate::layout::{Anchor, Anchors, Point};
use crate::script::object::anchor_args::{
    get_point_index, get_point_values, parse_set_all_points, resolve_rel_target,
};
use crate::script::object::frame_wrapper;
use crate::script::region_map::{set_shared, Side};
use crate::script::{Model, SCREEN};

use super::{
    measured_wh, region_handle_of, region_ladder_context, region_owner_id, region_set_point,
    size_bits_eq,
};

/// `ClearAllPoints` on a region (`0x767ed0` → `0x767620`): the region leaves the cached graph with
/// its edges, and keeps its rect, as the clear queues no resolve, until its own next invalidate.
pub(in crate::script) fn clear_points(model: &mut Model, rh: crate::widget::RegionHandle) {
    // Mutates only on a real change.
    if model.region_data.entry(rh).or_default().anchors.clear() {
        model.touch_layout_region(rh);
    }
}

/// Install the layout methods into `m`.
pub(super) fn install(lua: &Lua, m: &Table) -> mlua::Result<()> {
    // The explicit size fills the axes the anchors do not pin, so SetAllPoints's two corners
    // leave it unread.
    set_shared(
        lua,
        m,
        Side::Region,
        "SetWidth",
        |lua, (this, w): (Table, f32)| {
            let rh = region_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let d = model.region_data.entry(rh).or_default();
            let new = Some((w, d.size.map_or(0.0, |s| s.1)));
            let changed = !size_bits_eq(d.size, new);
            d.size = new;
            // No edge or roster change; the width, as wrap width, keys a FontString's measure. It
            // invalidates whether or not the size moved (`0x7683d0`, `0x7683f0`).
            if changed {
                model.touch_measure(rh);
            }
            model.invalidate_region(rh, changed);
            Ok(())
        },
    )?;

    set_shared(
        lua,
        m,
        Side::Region,
        "SetHeight",
        |lua, (this, h): (Table, f32)| {
            let rh = region_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let d = model.region_data.entry(rh).or_default();
            let new = Some((d.size.map_or(0.0, |s| s.0), h));
            let changed = !size_bits_eq(d.size, new);
            d.size = new;
            // No edge or roster change; the width, as wrap width, keys a FontString's measure. It
            // invalidates whether or not the size moved (`0x7683d0`, `0x7683f0`).
            if changed {
                model.touch_measure(rh);
            }
            model.invalidate_region(rh, changed);
            Ok(())
        },
    )?;

    // No `SetSize`: not a 1.12 verb.

    // The frame's `GetWidth`/`GetHeight` (`0x7a1e00`, `0x7a2030`): the region's own size slot
    // ([`measured_wh`]) as stored, when non-zero, with no resolve; else, resolving a pending region,
    // its rect's span, not divided by the owner's effective scale, or 0 with no rect; each through
    // the getter steps.
    set_shared(lua, m, Side::Region, "GetWidth", |lua, this: Table| {
        region_size_read(lua, &this, true)
    })?;

    set_shared(lua, m, Side::Region, "GetHeight", |lua, this: Table| {
        region_size_read(lua, &this, false)
    })?;

    // GetLeft/GetRight/GetTop/GetBottom: the resolved edges over the owner's effective scale,
    // which a region shares, through the getter steps (y up); nil while the valid bit is clear.
    // The frame's getters (`0x7a1980`..), so they resolve first, as a frame's do.
    for (name, pick) in [
        ("GetLeft", 0u8),
        ("GetRight", 1u8),
        ("GetTop", 2u8),
        ("GetBottom", 3u8),
    ] {
        set_shared(lua, m, Side::Region, name, move |lua, this: Table| {
            let rh = region_handle_of(lua, &this)?;
            crate::script::object::settle_region(lua, rh);
            let model = lua.app_data_ref::<Model>().expect("model");
            let (units, s) = (model.layout_units(), owner_scale(&model, rh));
            Ok(model.region_rects.get(&rh).map(|r| {
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

    // SetPoint, ClearAllPoints and SetAllPoints write the region's anchors. `relativeTo` defaults
    // to the owner frame, and a named one may be a frame or a sibling region, as stock XML uses.
    set_shared(
        lua,
        m,
        Side::Region,
        "SetPoint",
        |lua, (this, rest): (Table, MultiValue)| region_set_point(lua, &this, &rest),
    )?;

    set_shared(
        lua,
        m,
        Side::Region,
        "ClearAllPoints",
        |lua, this: Table| {
            let rh = region_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            clear_points(&mut model, rh);
            Ok(())
        },
    )?;

    set_shared(
        lua,
        m,
        Side::Region,
        "SetAllPoints",
        |lua, (this, rest): (Table, MultiValue)| {
            let rh = region_handle_of(lua, &this)?;
            // `who` and `$parent` first, then the `_G` read, then the guard.
            let (who, base) = region_ladder_context(lua, rh);
            let target = parse_set_all_points(lua, rest.front(), &base);
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let me = model.region_id(rh);
            let owner = region_owner_id(&mut model, rh);
            let rel_id = resolve_rel_target(&model, &target, &who, "SetAllPoints", me, owner)?;
            let pair = [
                Anchor::new(Point::TopLeft, rel_id, Point::TopLeft, 0.0, 0.0),
                Anchor::new(Point::BottomRight, rel_id, Point::BottomRight, 0.0, 0.0),
            ];
            let data = model.region_data.entry(rh).or_default();
            // The reference clears, sets both and invalidates (`0x767db0`); a bit-identical pair
            // moves no input.
            let pair = Anchors::from(pair);
            let changed = !data.anchors.bits_eq(&pair);
            data.anchors = pair;
            model.invalidate_region(rh, changed);
            Ok(())
        },
    )?;

    // ── the rest of the Region map (`0xcf54b4`) ─────────────────────────────────────────────────
    //
    // The map is a closed set, `script::REGION_MAP_METHODS`, and a test asserts it whole.

    // GetParent: the owner frame, which a region always has, so never nil.
    set_shared(lua, m, Side::Region, "GetParent", |lua, this: Table| {
        let rh = region_handle_of(lua, &this)?;
        let owner = {
            let mut model = lua.app_data_mut::<Model>().expect("model");
            region_owner_id(&mut model, rh)
        };
        frame_wrapper(lua, owner)
    })?;

    // GetCenter: the frame's binding (`0x7a1810`), the resolved rect's midpoints over the owner's
    // scale through the getter steps, or two nils while the valid bit is clear; it resolves first,
    // as the edge readers do.
    set_shared(lua, m, Side::Region, "GetCenter", |lua, this: Table| {
        let rh = region_handle_of(lua, &this)?;
        crate::script::object::settle_region(lua, rh);
        let model = lua.app_data_ref::<Model>().expect("model");
        let (units, s) = (model.layout_units(), owner_scale(&model, rh));
        Ok(match model.region_rects.get(&rh) {
            Some(r) => (
                Value::Number(units.center(r.left, r.right, s)),
                Value::Number(units.center(r.bottom, r.top, s)),
            ),
            None => (Value::Nil, Value::Nil),
        })
    })?;

    set_shared(lua, m, Side::Region, "GetNumPoints", |lua, this: Table| {
        let rh = region_handle_of(lua, &this)?;
        let model = lua.app_data_ref::<Model>().expect("model");
        Ok(model
            .region_data
            .get(&rh)
            .map_or(0, |d| d.anchors.len() as i64))
    })?;

    // GetPoint([n]): the frame's binding (`0x7a2340`), the n-th anchor in point order. Frames and
    // regions share one id space and a region may anchor to a sibling region, so relativeTo is
    // answered with the matching wrapper kind.
    set_shared(
        lua,
        m,
        Side::Region,
        "GetPoint",
        |lua, (this, n): (Table, Value)| {
            let rh = region_handle_of(lua, &this)?;
            let idx = get_point_index(lua, n)?;
            let (anchor, units) = {
                let model = lua.app_data_ref::<Model>().expect("model");
                let anchor = model
                    .region_data
                    .get(&rh)
                    .and_then(|d| d.anchors.nth(idx))
                    .copied();
                (anchor, model.layout_units())
            };
            get_point_values(lua, anchor, units, |rel| {
                let is_frame = {
                    let model = lua.app_data_ref::<Model>().expect("model");
                    model.id_to_frame.contains_key(&rel)
                };
                Ok(if rel == SCREEN {
                    Value::Nil
                } else if is_frame {
                    Value::Table(frame_wrapper(lua, rel)?)
                } else {
                    Value::Table(super::region_wrapper(lua, rel)?)
                })
            })
        },
    )?;
    Ok(())
}

/// The region's layout scale, its owner frame's effective scale (1 for an orphan), which the edge
/// and centre getters divide by.
fn owner_scale(model: &Model, rh: crate::widget::RegionHandle) -> f32 {
    model
        .arena
        .region(rh)
        .map(|r| crate::script::object::layout_scale(model, r.owner))
        .unwrap_or(1.0)
}

/// The region side of `GetWidth`/`GetHeight`; see the bindings. The slot answers in layout units
/// and `fst` stores it as a float before the getter steps (`0x7a1e99`), so a texture's art extent
/// and a font string's floor read back as `G(S(v))` of their size in script units.
fn region_size_read(lua: &Lua, this: &Table, width: bool) -> mlua::Result<f64> {
    let (w, h) = measured_wh(lua, this)?;
    let set = if width { w } else { h };
    let rh = region_handle_of(lua, this)?;
    let (units, text) = {
        let model = lua.app_data_ref::<Model>().expect("model");
        let text = model
            .arena
            .region(rh)
            .is_some_and(|r| matches!(r.kind, crate::widget::RegionKind::FontString));
        (model.layout_units(), text)
    };
    let stored = units.store(set);
    // A FontString's size is floored at one unit (`0x772930`, `0x772a60`), never 0, so it never
    // reads its rect; its 0 is a host measure still in flight, which answers 0.
    if stored != 0.0 || text {
        return Ok(units.load(stored));
    }
    crate::script::object::settle_region(lua, rh);
    let model = lua.app_data_ref::<Model>().expect("model");
    Ok(model.region_rects.get(&rh).map_or(0.0, |r| {
        if width {
            units.span(r.left, r.right)
        } else {
            units.span(r.bottom, r.top)
        }
    }))
}
