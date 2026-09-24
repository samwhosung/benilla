//! The Region layout methods: size, anchors and the resolved-rect readers.

use mlua::{Lua, MultiValue, Table, Value};

use crate::layout::{Anchor, Point};
use crate::script::object::anchor_args::{parse_set_all_points, resolve_rel_target};
use crate::script::object::{anchor_bits_eq, frame_wrapper, point_name};
use crate::script::region_map::{set_shared, Side};
use crate::script::{Model, SCREEN};

use super::{
    measured_wh, region_handle_of, region_ladder_context, region_owner_id, region_set_point,
    size_bits_eq,
};

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
            if changed {
                // No edge or roster change; the width, as wrap width, keys a FontString's measure.
                model.touch_layout_region(rh);
                model.touch_measure(rh);
            }
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
            if changed {
                // No edge or roster change; the width, as wrap width, keys a FontString's measure.
                model.touch_layout_region(rh);
                model.touch_measure(rh);
            }
            Ok(())
        },
    )?;

    // No `SetSize`: not a 1.12 verb.

    set_shared(lua, m, Side::Region, "GetWidth", |lua, this: Table| {
        Ok(measured_wh(lua, &this)?.0)
    })?;

    set_shared(lua, m, Side::Region, "GetHeight", |lua, this: Table| {
        Ok(measured_wh(lua, &this)?.1)
    })?;

    // GetLeft/GetRight/GetTop/GetBottom: the resolved edges in the owner's units (y up, screen
    // over the owner's effective scale, which a region shares); nil for a region never resolved.
    for (name, pick) in [
        ("GetLeft", 0u8),
        ("GetRight", 1u8),
        ("GetTop", 2u8),
        ("GetBottom", 3u8),
    ] {
        set_shared(lua, m, Side::Region, name, move |lua, this: Table| {
            let rh = region_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            let inv = 1.0 / owner_scale(&model, rh);
            Ok(model.region_resolved.get(&rh).map(|r| {
                inv * match pick {
                    0 => r.left,
                    1 => r.right,
                    2 => r.top,
                    _ => r.bottom,
                }
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
            let d = model.region_data.entry(rh).or_default();
            // A retarget onto no targets: the cached graph unlinks the edges, no re-derive.
            let old: Option<Vec<u32>> =
                (!d.anchors.is_empty()).then(|| d.anchors.iter().map(|a| a.relative_to).collect());
            d.anchors.clear();
            if let Some(old) = old {
                model.touch_layout_retarget_region(rh, &old, &[]);
            }
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
            let same = data.anchors.len() == 2
                && data
                    .anchors
                    .iter()
                    .zip(&pair)
                    .all(|(a, b)| anchor_bits_eq(a, b));
            if !same {
                data.anchors.clear();
                data.anchors.extend_from_slice(&pair);
                model.touch_layout();
            }
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

    // GetCenter: the resolved rect's midpoint in the owner's units, as the edge readers answer, or
    // two nils before the first resolve.
    set_shared(lua, m, Side::Region, "GetCenter", |lua, this: Table| {
        let rh = region_handle_of(lua, &this)?;
        let model = lua.app_data_ref::<Model>().expect("model");
        let inv = 1.0 / owner_scale(&model, rh);
        Ok(match model.region_resolved.get(&rh) {
            Some(r) => (
                Value::Number(f64::from(inv * (r.left + r.right) * 0.5)),
                Value::Number(f64::from(inv * (r.bottom + r.top) * 0.5)),
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

    // GetPoint([n]): the n-th anchor (1-based, default 1) as point, relativeTo, relativePoint, x
    // and y, or five nils. Frames and regions share one id space and a region may anchor to a
    // sibling region, so relativeTo is answered with the matching wrapper kind.
    set_shared(
        lua,
        m,
        Side::Region,
        "GetPoint",
        |lua, (this, n): (Table, Option<i64>)| {
            let rh = region_handle_of(lua, &this)?;
            let anchor = {
                let model = lua.app_data_ref::<Model>().expect("model");
                let idx = (n.unwrap_or(1).max(1) - 1) as usize;
                model
                    .region_data
                    .get(&rh)
                    .and_then(|d| d.anchors.get(idx))
                    .cloned()
            };
            let Some(a) = anchor else {
                return Ok((Value::Nil, Value::Nil, Value::Nil, Value::Nil, Value::Nil));
            };
            let rel = {
                let is_frame = {
                    let model = lua.app_data_ref::<Model>().expect("model");
                    model.id_to_frame.contains_key(&a.relative_to)
                };
                if a.relative_to == SCREEN {
                    Value::Nil
                } else if is_frame {
                    Value::Table(frame_wrapper(lua, a.relative_to)?)
                } else {
                    Value::Table(super::region_wrapper(lua, a.relative_to)?)
                }
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
    Ok(())
}

/// The owner frame's effective scale (1 for an orphan), the divisor from a resolved screen rect to
/// the owner's units every region getter answers in.
fn owner_scale(model: &Model, rh: crate::widget::RegionHandle) -> f32 {
    model
        .arena
        .region(rh)
        .map(|r| crate::script::object::eff_scale(model, r.owner))
        .unwrap_or(1.0)
}
