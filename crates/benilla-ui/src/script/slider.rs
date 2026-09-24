//! The `Slider` methods of `CSimpleSlider` (factory `0x6eee40`, `Slider::LoadXML` `0x789580`): a
//! value in `[min, max]` with a step and an orientation (`0x811b00`), driving a thumb texture along
//! the track, whose placement follows the documented widget model, untraced in the reference.
//! `SetValue` fires `OnValueChanged` (slot `+0x330`) on the first value and then only on a change
//! (`+0x314` bit 2, `0x789a06` in `SetValue` `0x789930`), which ends the loop between the stock
//! scrollbar's `OnValueChanged` and its ScrollFrame's `OnVerticalScroll`. The methods exist only on
//! Slider frames, as in the client's per-class method sets.

use mlua::{Lua, Table, Value};

use super::object::{draw_layer_from_str, frame_handle_of};
use super::pointer::point_in_rect;
use super::region::region_wrapper;
use super::{event, Model, RegionData};
use crate::layout::Rect;
use crate::order::DrawLayer;
use crate::widget::{
    slider_fraction, slider_grab, FrameHandle, KindState, RegionKind, SliderState,
};

pub(super) const REG_SLIDER_METHODS: &str = "__benilla_slider_methods";

/// Run `f` over a frame's Slider state under one short write borrow; errors unless `this` is a live
/// Slider, since a caller can lift the method table onto another frame.
fn with_slider<T>(
    lua: &Lua,
    this: &Table,
    f: impl FnOnce(&mut SliderState) -> T,
) -> mlua::Result<T> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let frame = model
        .arena
        .frame_mut(h)
        .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
    match &mut frame.kind_state {
        KindState::Slider(s) => Ok(f(s)),
        _ => Err(mlua::Error::runtime("not a Slider")),
    }
}

/// Get or create the thumb texture region and return its id; `layer` re-layers an existing thumb,
/// and a new one defaults to OVERLAY (`0x789580`).
fn ensure_thumb(lua: &Lua, this: &Table, layer: Option<DrawLayer>) -> mlua::Result<u32> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");

    let existing = match &model
        .arena
        .frame(h)
        .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?
        .kind_state
    {
        KindState::Slider(s) => s.thumb,
        _ => return Err(mlua::Error::runtime("not a Slider")),
    };

    let rh = match existing {
        Some(rh) => {
            if let (Some(l), Some(region)) = (layer, model.arena.region_mut(rh)) {
                region.draw_layer = l;
            }
            rh
        }
        None => {
            let rh = model
                .arena
                .create_region(
                    h,
                    RegionKind::Texture,
                    layer.unwrap_or(DrawLayer::Overlay),
                    0,
                )
                .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
            model.region_data.insert(rh, RegionData::default());
            model.touch_layout(); // a region entered the layout gate's read set
            if let Some(frame) = model.arena.frame_mut(h) {
                if let KindState::Slider(s) = &mut frame.kind_state {
                    s.thumb = Some(rh);
                }
            }
            rh
        }
    };
    Ok(model.region_id(rh))
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    m.set(
        "SetMinMaxValues",
        lua.create_function(|lua, (this, min, max): (Table, f32, f32)| {
            // A reversed pair is kept, not swapped (`0x789580`). A held value re-clamps and fires
            // on a move; a fresh slider with no value yet fires nothing (bit 2).
            let changed = with_slider(lua, &this, |s| s.set_min_max(min, max))?;
            fire_value_changed(lua, &this, changed)
        })?,
    )?;
    m.set(
        "GetMinMaxValues",
        lua.create_function(|lua, this: Table| with_slider(lua, &this, |s| (s.min, s.max)))?,
    )?;
    m.set(
        "SetValue",
        lua.create_function(|lua, (this, v): (Table, f32)| {
            let changed = with_slider(lua, &this, |s| s.store_value(v))?;
            fire_value_changed(lua, &this, changed)
        })?,
    )?;
    m.set(
        "GetValue",
        lua.create_function(|lua, this: Table| with_slider(lua, &this, |s| s.value))?,
    )?;
    m.set(
        "SetValueStep",
        // Stores the step, then re-pushes the range as `SetMinMaxValues` does, so a held value
        // moved onto the new step fires `OnValueChanged`.
        lua.create_function(|lua, (this, step): (Table, f32)| {
            let changed = with_slider(lua, &this, |s| s.set_value_step(step))?;
            fire_value_changed(lua, &this, changed)
        })?,
    )?;
    m.set(
        "GetValueStep",
        lua.create_function(|lua, this: Table| with_slider(lua, &this, |s| s.step))?,
    )?;
    m.set(
        "SetOrientation",
        lua.create_function(|lua, (this, o): (Table, String)| {
            let vertical = match o.to_ascii_uppercase().as_str() {
                "HORIZONTAL" => false,
                "VERTICAL" => true,
                _ => {
                    return Err(mlua::Error::runtime(format!(
                        "SetOrientation: unknown orientation '{o}'"
                    )))
                }
            };
            with_slider(lua, &this, |s| s.vertical = vertical)
        })?,
    )?;
    m.set(
        "GetOrientation",
        lua.create_function(|lua, this: Table| {
            let v = with_slider(lua, &this, |s| s.vertical)?;
            Ok(if v { "VERTICAL" } else { "HORIZONTAL" }.to_string())
        })?,
    )?;

    // A 1.12 Slider has no enabled state: `Enable`/`Disable`/`IsEnabled` exist only in the Button
    // table (`0x879d00`: `0x77fef0`, `0x77ffd0`, `0x7800b0`), so a Slider must not answer them.

    // `SetThumbTexture(path [, layer])` or `(r, g, b [, a])`, a region's two `SetTexture` forms, on
    // the thumb region, created on first use.
    m.set(
        "SetThumbTexture",
        lua.create_function(
            |lua, (this, a1, a2, a3, a4): (Table, Value, Value, Value, Value)| {
                let layer = match &a2 {
                    Value::String(s) => s.to_str().ok().and_then(|l| draw_layer_from_str(&l)),
                    _ => None,
                };
                let id = ensure_thumb(lua, &this, layer)?;
                let rh = {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    *model.id_to_region.get(&id).expect("thumb region id")
                };
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                let data = model.region_data.entry(rh).or_default();
                match &a1 {
                    Value::String(s) => {
                        data.texture = Some(s.to_str()?.to_string());
                        data.fill = None;
                    }
                    // A solid thumb and a path share one slot; each clears the other.
                    Value::Number(_) | Value::Integer(_) => {
                        data.texture = None;
                        data.fill = Some([
                            num_f32(&a1),
                            num_f32(&a2),
                            num_f32(&a3),
                            match &a4 {
                                Value::Nil => 1.0,
                                v => num_f32(v),
                            },
                        ]);
                    }
                    _ => {}
                }
                Ok(())
            },
        )?,
    )?;
    m.set(
        "GetThumbTexture",
        lua.create_function(|lua, this: Table| {
            let thumb = with_slider(lua, &this, |s| s.thumb)?;
            let id = {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                thumb.map(|rh| model.region_id(rh))
            };
            match id {
                Some(id) => Ok(Value::Table(region_wrapper(lua, id)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    lua.set_named_registry_value(REG_SLIDER_METHODS, m)?;
    Ok(())
}

/// Fire `OnValueChanged` (slot `+0x330`) when `changed` carries a new value, outside any model
/// borrow; errors are recorded in [`Model::errors`].
fn fire_value_changed(lua: &Lua, this: &Table, changed: Option<f32>) -> mlua::Result<()> {
    let Some(value) = changed else { return Ok(()) };
    let id = {
        let h = frame_handle_of(lua, this)?;
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        model.frame_id(h)
    };
    if let Err(e) = event::fire_widget_handler(
        lua,
        id,
        "OnValueChanged",
        vec![Value::Number(f64::from(value))],
    ) {
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .record_script_error(e.to_string());
    }
    Ok(())
}

fn num_f32(v: &Value) -> f32 {
    match v {
        Value::Number(n) => *n as f32,
        Value::Integer(i) => *i as f32,
        _ => 0.0,
    }
}

// ── Thumb geometry and drag ──────────────────────────────────────────────────────────────────

/// The thumb's rect at `fraction` along the track, centred across it and inset so it travels
/// `trackLen - thumbLen`; vertical puts fraction 0 at the top. Render and [`begin_drag`] both use
/// it, so they never disagree. `thumb_size` is [`thumb_extent`]'s.
pub(super) fn thumb_rect(r: Rect, thumb_size: (f32, f32), vertical: bool, fraction: f32) -> Rect {
    let (tw, th) = thumb_size;
    if vertical {
        let travel = (r.height() - th).max(0.0);
        let top = r.top - fraction * travel; // f=0 → track top, f=1 → track bottom
        let cx = (r.left + r.right) * 0.5;
        Rect::new(top - th, cx - tw * 0.5, top, cx + tw * 0.5)
    } else {
        let travel = (r.width() - tw).max(0.0);
        let left = r.left + fraction * travel;
        let cy = (r.bottom + r.top) * 0.5;
        Rect::new(cy - th * 0.5, left, cy + th * 0.5, left + tw)
    }
}

/// The thumb region's own width and height, as `0x789ba0` reads them through the texture's
/// geometry slots (`[vtable+0x1c]`/`+0x20`, `0x770720`/`0x770790`): the authored span, else the
/// art's texel span, else 0. `None` without a thumb region: `0x789ba0` gates the value math on
/// `+0x328`, so a thumbless slider captures a press but never moves.
fn thumb_extent(model: &Model, thumb: Option<crate::widget::RegionHandle>) -> Option<(f32, f32)> {
    Some(super::region::virtual_span(model, thumb?))
}

/// The in-flight thumb drag and the grab offset [`slider_grab`] returned, so the thumb tracks the
/// cursor without jumping.
#[derive(Clone, Copy)]
pub(crate) struct SliderDrag {
    pub(crate) slider: FrameHandle,
    /// The grab as distance from the track's leading edge, the axis-neutral frame [`slider_grab`]
    /// and [`slider_fraction`] share with the glue screens.
    pub(crate) grab_offset: f32,
}

/// On a LeftButton press on a Slider `hit`, capture a drag at the [`slider_grab`] offset, with no
/// enabled gate since a 1.12 Slider has no enabled state. Returns `(frame id, new value)` when the
/// press jumps the value, for the caller to fire `OnValueChanged` outside the model borrow.
pub(super) fn begin_drag(
    model: &mut Model,
    hit: Option<FrameHandle>,
    x: f32,
    y: f32,
) -> Option<(u32, f32)> {
    let h = hit?;
    let r = model.resolved.get(&h).copied()?;
    let (vertical, fraction, thumb) = match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(KindState::Slider(s)) => (s.vertical, s.fraction(), s.thumb),
        _ => return None,
    };
    // No thumb region: the press still captures, the dispatcher's store being unconditional, but
    // the value never moves (`0x789ba0`, `+0x328`).
    let Some(size) = thumb_extent(model, thumb) else {
        model.slider_drag = Some(SliderDrag {
            slider: h,
            grab_offset: 0.0,
        });
        return None;
    };
    let trect = thumb_rect(r, size, vertical, fraction);
    let on_thumb = point_in_rect(trect, x, y);
    // Distances from the track's leading edge, where the thumb sits at `min`: `top - y` on a
    // vertical track in this y-up arena, `x - left` on a horizontal one.
    let (cursor, thumb_lead, thumb_len) = if vertical {
        (r.top - y, r.top - trect.top, trect.top - trect.bottom)
    } else {
        (x - r.left, trect.left - r.left, trect.right - trect.left)
    };
    model.slider_drag = Some(SliderDrag {
        slider: h,
        grab_offset: slider_grab(cursor, thumb_lead, thumb_len),
    });
    if on_thumb {
        return None; // no value change from the grab itself
    }
    drag_move(model, x, y)
}

/// On a pointer move during a capture, set the value from the cursor through [`slider_fraction`];
/// returns `(frame id, new value)` when it changed, for the caller to fire.
pub(super) fn drag_move(model: &mut Model, x: f32, y: f32) -> Option<(u32, f32)> {
    let SliderDrag {
        slider,
        grab_offset,
    } = *model.slider_drag.as_ref()?;
    let r = model.resolved.get(&slider).copied()?;
    let (min, max, vertical, thumb) = match model.arena.frame(slider).map(|f| &f.kind_state) {
        Some(KindState::Slider(s)) => (s.min, s.max, s.vertical, s.thumb),
        _ => return None, // slider destroyed mid-drag
    };
    let (tw, th) = thumb_extent(model, thumb)?;
    // The leading-edge frame `begin_drag` stored the grab in.
    let (cursor, track_extent, thumb_len) = if vertical {
        (r.top - y, r.height(), th)
    } else {
        (x - r.left, r.width(), tw)
    };
    // `None` when the thumb is as long as its track.
    let fraction = slider_fraction(cursor, grab_offset, track_extent, thumb_len)?;
    let value = min + fraction * (max - min);
    let changed = match model.arena.frame_mut(slider).map(|f| &mut f.kind_state) {
        Some(KindState::Slider(s)) => s.store_value(value),
        _ => None,
    };
    changed.map(|v| (model.frame_id(slider), v))
}

/// Release any in-flight thumb drag (LeftButton up, or the pointer leaving the window).
pub(super) fn end_drag(model: &mut Model) {
    model.slider_drag = None;
}
