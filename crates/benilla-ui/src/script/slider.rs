//! The `Slider` methods of `CSimpleSlider` (factory `0x6eee40`, `Slider::LoadXML` `0x789580`): a
//! value in `[min, max]` with a step and an orientation (`0x811b00`), driving a thumb texture along
//! the track, whose placement follows the documented widget model, untraced in the reference.
//! `SetValue` fires `OnValueChanged` (slot `+0x330`) on the first value and then only on a change
//! (`+0x314` bit 2, `0x789a06` in `SetValue` `0x789930`), which ends the loop between the stock
//! scrollbar's `OnValueChanged` and its ScrollFrame's `OnVerticalScroll`. The methods exist only on
//! Slider frames, as in the client's per-class method sets.

use mlua::{Lua, Table, Value};

use super::binding_abi;
use super::object::frame_handle_of;
use super::pointer::point_in_rect;
use super::region::{install_into_slot, load_path_leg, region_wrapper, slot_arg, SlotArg};
use super::{event, Model};
use crate::layout::Rect;
use crate::order::DrawLayer;
use crate::widget::{
    slider_fraction, slider_grab, FrameHandle, KindState, RegionHandle, SliderState,
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

/// Install `incoming` as the thumb on `layer`, or clear it, through `0x789860`: the same texture
/// is a no-op, the displaced one is deleted, the incoming one is linked in shown and loses its
/// anchors (`0x789893`), the widget placing it.
fn install_thumb_model(
    model: &mut Model,
    h: FrameHandle,
    incoming: Option<RegionHandle>,
    layer: DrawLayer,
) -> mlua::Result<()> {
    if !matches!(
        model.arena.frame(h).map(|f| &f.kind_state),
        Some(KindState::Slider(_))
    ) {
        return Err(mlua::Error::runtime("not a Slider"));
    }
    install_into_slot(
        model,
        h,
        incoming,
        layer,
        true,
        true,
        |ks| match ks {
            KindState::Slider(sl) => sl.thumb,
            _ => None,
        },
        |ks, rh| {
            if let KindState::Slider(sl) = ks {
                sl.thumb = rh;
            }
        },
    )
    .map(|_| ())
}

/// `<ThumbTexture>` (`0x7895f6`): the texture the texture adder built, installed as an object on
/// the element's `drawLayer`, OVERLAY without one.
pub(super) fn install_loaded(
    model: &mut Model,
    h: FrameHandle,
    rh: RegionHandle,
    layer: Option<DrawLayer>,
) -> mlua::Result<()> {
    install_thumb_model(model, h, Some(rh), layer.unwrap_or(DrawLayer::Overlay))
}

/// `SetThumbTexture`'s path leg (`0x7897c0`, [`load_path_leg`]): a thumb that exists has its file
/// swapped and nothing else; with none, a texture that loads is installed on OVERLAY
/// (`0x790431`'s layer).
fn set_thumb_path(lua: &Lua, this: &Table, path: &str) -> mlua::Result<()> {
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
    if let Some(rh) = load_path_leg(&mut model, h, existing, DrawLayer::Overlay, path)? {
        install_thumb_model(&mut model, h, Some(rh), DrawLayer::Overlay)?;
    }
    Ok(())
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    m.set(
        "SetMinMaxValues",
        lua.create_function(|lua, (this, min, max): (Table, Value, Value)| {
            // Both `lua_isnumber` (`0x790832`, `0x790843`) else
            // `Usage: %s:SetMinMaxValues(min, max)` (`0x87b160`).
            with_slider(lua, &this, |_| ())?;
            let line = "SetMinMaxValues(min, max)";
            let min = binding_abi::method_number(lua, &this, min, line)? as f32;
            let max = binding_abi::method_number(lua, &this, max, line)? as f32;
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
        lua.create_function(|lua, (this, v): (Table, Value)| {
            // `lua_isnumber` (`0x790a10`) else `Usage: %s:SetValue(value)` (`0x87b184`).
            with_slider(lua, &this, |_| ())?;
            let v = binding_abi::method_number(lua, &this, v, "SetValue(value)")? as f32;
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
        // `lua_isnumber` (`0x790bc0`) else `Usage: %s:SetValueStep(value)` (`0x87b39c`). Stores
        // the step, then re-pushes the range as `SetMinMaxValues` does, so a held value moved onto
        // the new step fires `OnValueChanged`.
        lua.create_function(|lua, (this, step): (Table, Value)| {
            with_slider(lua, &this, |_| ())?;
            let step = binding_abi::method_number(lua, &this, step, "SetValueStep(value)")? as f32;
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
        lua.create_function(|lua, (this, o): (Table, Value)| {
            with_slider(lua, &this, |_| ())?;
            let vertical = binding_abi::orientation_arg(lua, &this, &o)?;
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

    // `SetThumbTexture(texture or "texture" or nil)` (`0x7902b0`): a Texture object is installed as
    // the thumb, linked on OVERLAY (the constant `0x790431` passes) with its anchors cleared
    // (`0x789893`), displacing the old thumb; a string or a number is the thumb's path
    // (`0x790442`), and nil clears it (`0x790473`). It reads no layer and has no colour form.
    m.set(
        "SetThumbTexture",
        lua.create_function(|lua, (this, args): (Table, mlua::MultiValue)| {
            match slot_arg(
                lua,
                &this,
                &args,
                r#"SetThumbTexture(texture or "texture" or nil)"#,
                "SetThumbTexture",
                true,
            )? {
                SlotArg::Object(rh) => {
                    let h = frame_handle_of(lua, &this)?;
                    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                    install_thumb_model(&mut model, h, Some(rh), DrawLayer::Overlay)
                }
                SlotArg::Clear => {
                    let h = frame_handle_of(lua, &this)?;
                    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                    install_thumb_model(&mut model, h, None, DrawLayer::Overlay)
                }
                SlotArg::Path(path) => set_thumb_path(lua, &this, &path),
            }
        })?,
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
            .record_lua_error(&e);
    }
    Ok(())
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
/// art's texel span, else 0, in the slider's units, unscaled, though the track it is subtracted
/// from is the scaled rect (`0x789bd4`–`0x789bfe`). `None` without a thumb region: `0x789ba0`
/// gates the value math on `+0x328`, so a thumbless slider captures a press but never moves.
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
    // The thumb as drawn, at the slider's scale, decides whether the press is on it.
    let drawn = thumb.map_or(size, |t| super::region::screen_span(model, t));
    let trect = thumb_rect(r, drawn, vertical, fraction);
    let on_thumb = point_in_rect(trect, x, y);
    // Distances from the track's leading edge, where the thumb sits at `min`: `top - y` on a
    // vertical track in this y-up arena, `x - left` on a horizontal one.
    let (cursor, thumb_lead, drawn_len, own_len) = if vertical {
        (
            r.top - y,
            r.top - trect.top,
            trect.top - trect.bottom,
            size.1,
        )
    } else {
        (
            x - r.left,
            trect.left - r.left,
            trect.right - trect.left,
            size.0,
        )
    };
    // Off the thumb the cursor takes its centre, half its own unscaled length (`0x789bf5`).
    let grab_offset = if on_thumb {
        slider_grab(cursor, thumb_lead, drawn_len)
    } else {
        own_len * 0.5
    };
    model.slider_drag = Some(SliderDrag {
        slider: h,
        grab_offset,
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
