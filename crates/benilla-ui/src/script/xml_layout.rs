//! The frame loader's layout-unit lengths. Its length readers store an `<Abs…>` child or the node's
//! own attributes by `S` and a `<Rel…>` child's numbers as parsed (`XmlReadDimension`
//! `0x6f2037`-`0x6f2060`), and the loader writes what they answer straight into the cells. The
//! loader reaches the cells through the script verbs, which store a script number; for the cells
//! benilla keeps as [`Len`], these put a `<Rel…>` axis back in layout units over what the verb
//! stored.

use mlua::{Lua, Table};

use super::object::{frame_handle_of, point_from_str};
use super::region::region_handle_of;
use super::Model;
use crate::layout::Len;
use crate::widget::KindState;

fn is_layout(v: Len) -> bool {
    matches!(v, Len::Layout(_))
}

/// Put each layout axis of `v` over `cell`.
fn put(cell: (&mut Len, &mut Len), v: [Len; 2]) {
    if is_layout(v[0]) {
        *cell.0 = v[0];
    }
    if is_layout(v[1]) {
        *cell.1 = v[1];
    }
}

/// An `<Anchor>`'s `<Offset>` (`0x7679ad` → `0x767c70`), after its `SetPoint`.
pub(crate) fn offset(lua: &Lua, wrapper: &Table, region: bool, point: &str, v: [Len; 2]) {
    if !v.iter().any(|&l| is_layout(l)) {
        return;
    }
    let Some(p) = point_from_str(point) else {
        return;
    };
    if region {
        let Ok(rh) = region_handle_of(lua, wrapper) else {
            return;
        };
        let mut model = lua.app_data_mut::<Model>().expect("model");
        if let Some(cell) = model
            .region_data
            .get_mut(&rh)
            .and_then(|d| d.anchors.offset_mut(p))
        {
            put(cell, v);
            model.invalidate_region(rh, true);
        }
    } else {
        let Ok(h) = frame_handle_of(lua, wrapper) else {
            return;
        };
        let mut model = lua.app_data_mut::<Model>().expect("model");
        if let Some(cell) = model
            .layout_inputs
            .get_mut(&h)
            .and_then(|i| i.anchors.offset_mut(p))
        {
            put(cell, v);
            model.invalidate_frame(h, true);
        }
    }
}

/// A `<Size>` (`0x767835`-`0x76784a`), after its `SetWidth` and `SetHeight`.
pub(crate) fn size(lua: &Lua, wrapper: &Table, region: bool, v: [Len; 2]) {
    if !v.iter().any(|&l| is_layout(l)) {
        return;
    }
    if region {
        let Ok(rh) = region_handle_of(lua, wrapper) else {
            return;
        };
        let mut model = lua.app_data_mut::<Model>().expect("model");
        let Some(d) = model.region_data.get_mut(&rh) else {
            return;
        };
        if let Some((w, h)) = d.size.as_mut() {
            put((w, h), v);
            model.touch_measure(rh);
            model.invalidate_region(rh, true);
        }
    } else {
        let Ok(h) = frame_handle_of(lua, wrapper) else {
            return;
        };
        let mut model = lua.app_data_mut::<Model>().expect("model");
        let input = model.layout_inputs.entry(h).or_default();
        put((&mut input.width, &mut input.height), v);
        model.invalidate_frame(h, true);
    }
}

/// A `<PushedTextOffset>` (`0x778aec` → `0x778b24`), after its `SetPushedTextOffset`.
pub(crate) fn pushed_text_offset(lua: &Lua, wrapper: &Table, v: [Len; 2]) {
    if !v.iter().any(|&l| is_layout(l)) {
        return;
    }
    let Ok(h) = frame_handle_of(lua, wrapper) else {
        return;
    };
    let mut model = lua.app_data_mut::<Model>().expect("model");
    if let Some(KindState::Button(bs)) = model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
        let [x, y] = &mut bs.pushed_text_offset;
        put((x, y), v);
    }
}

/// A `<Backdrop>`'s `<EdgeSize>` (`0x77e7ae`), after its `SetBackdrop`.
pub(crate) fn edge_size(lua: &Lua, wrapper: &Table, v: Len) {
    if !is_layout(v) {
        return;
    }
    let Ok(h) = frame_handle_of(lua, wrapper) else {
        return;
    };
    let mut model = lua.app_data_mut::<Model>().expect("model");
    let Some(spec) = model
        .backdrops
        .get(&h)
        .map(|i| (i.spec.tile_size, i.spec.insets))
    else {
        return;
    };
    super::backdrop::set_metrics(&mut model, h, v, spec.0, spec.1);
}
