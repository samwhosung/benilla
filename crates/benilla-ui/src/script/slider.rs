//! The `Slider` method surface — the `CSimpleSlider` widget behavior over the kind tag (factory
//! `0x6eee40`). A value in `[min, max]` with a step and orientation, driving a thumb texture along
//! the track.
//!
//! Grounded in the byte-verified LoadXML table (`Slider::LoadXML 0x789580`): the
//! `<ThumbTexture>` sub-element (default layer OVERLAY=3), `minValue`/`maxValue`/`valueStep`/
//! `defaultValue`, the `orientation` shared enum (HORIZONTAL=0/VERTICAL=1, `0x811b00`), and the
//! widget's own `OnValueChanged` script slot (`+0x330`). The **thumb-position** mechanism — the
//! thumb's rect placed at the value fraction along the orientation axis, applied at extract — is
//! faithful to the *documented* widget model, not byte-pinned (same posture as StatusBar's fill and
//! ScrollFrame's scroll; decisions 0112/0250).
//!
//! `SetValue` fires `OnValueChanged` on the **first-ever** value and after that **only on an
//! actual change** ([`SliderState::store_value`] — the client's `+0x314` bit2, `SetValue
//! 0x789930`'s `0x789a06 test bl,4` gate). The change-gate is load-bearing, not an optimization:
//! the real scrollbar template wires `OnValueChanged →
//! this:GetParent():SetVerticalScroll(arg1)` and the ScrollFrame's `OnVerticalScroll →
//! scrollbar:SetValue(arg1)` back the other way, so a fire-always `SetValue` would recurse
//! forever (`UIPanelTemplates.xml`); the gate breaks the loop after one hop. `SetMinMaxValues`
//! re-clamps through the same gate only once a value exists, so a range set from `<OnLoad>` never
//! runs a handler the addon has not armed yet.
//!
//! The methods live in their own registry table, consulted by the frame `__index` dispatcher only
//! for Slider frames — so duck-typing addons (`if frame:GetThumbTexture() then …`) see `nil` on
//! every other kind, exactly as against the client's per-class method sets.

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

/// Registry key of the Slider method table (the MAXCSTACK discipline: Lua-side root, named key).
pub(super) const REG_SLIDER_METHODS: &str = "__benilla_slider_methods";

/// Run `f` over a frame's Slider state under one short write borrow. Errors if `this` is not a live
/// Slider (unreachable through the kind dispatcher, but the method table is a plain Lua value — a
/// caller could fish it out and misapply it).
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

/// Get-or-create the thumb texture region (`SetThumbTexture`/`<ThumbTexture>`); `layer` re-layers an
/// existing thumb. Returns the region's id (for wrapper lookup). The widget default layer is OVERLAY
/// (`Slider::LoadXML 0x789580` — the Slider's `drawLayer` attr defaults OVERLAY=3, unlike
/// StatusBar's ARTWORK bar).
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
            model.touch_layout(); // a region entered the layout gate's read set (decision 0740)
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
            // Unlike StatusBar, a reversed pair is NOT swapped (the Slider LoadXML `0x789580`
            // stores min + (max−min) and does no swap). A held value re-clamps into the new
            // range and a move fires OnValueChanged — but only once a value EXISTS
            // (`SliderState::set_min_max`, the client's bit2): on a fresh slider this sets the
            // range and fires nothing.
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
        // Not a field write (2143): past the step store it re-pushes the range through
        // `SetMinMaxValues`, which re-clamps and re-quantises the held value onto the new lattice
        // and fires `OnValueChanged` if it moved — so this binding fires like the other two.
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

    // `Enable`/`Disable`/`IsEnabled` WERE here and are GONE, with the `enabled` state they drove.
    // **A 1.12 Slider has no enabled state at all**: the three names are registered once each, in
    // the BUTTON table `0x879d00` (`0x77fef0`/`0x77ffd0`/`0x7800b0`), and the Slider's own LoadXML
    // `0x789580` takes `drawLayer`/`minValue`/`maxValue`/`valueStep`/`defaultValue`/`orientation`
    // and nothing else — so there was no way in
    // from Lua *or* from XML, and the flag could only ever read `true`.
    //
    // They were a superset in PRESENCE, which is what 1189 records the cost of: a duck-typing addon
    // that branches on `if widget.IsEnabled then` reads a Slider as a Button. Ours also answered a
    // Lua BOOLEAN, which the reference has no query that does (decision 2118) — `IsEnabled 0x7800b0`
    // is number-1/number-0 and never even nil.
    //
    // Removed rather than corrected because nothing calls them on a Slider receiver: a
    // receiver-typed grep over this repo, `assets/ui`, the stock FrameXML/GlueXML and both addon
    // corpora (110 top-20 + 219 vanilla) finds zero `slider*:Enable/Disable/IsEnabled` sites.
    // `begin_drag` below lost its `enabled` gate with them — behaviour-neutral, since no caller
    // could clear the flag.

    // SetThumbTexture(path [, drawLayer]) | SetThumbTexture(r, g, b [, a]) — the same two forms as a
    // region's SetTexture, targeting the thumb region (created on first use). Mirrors
    // SetStatusBarTexture; the thumb defaults to the OVERLAY layer.
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
                    // A solid thumb writes the same slot the path form does — each clears the other.
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

/// Fire `OnValueChanged(self, value)` if `changed` carries the new value (`Slider::LoadXML
/// 0x789580`: the Slider's own script slot `+0x330`). Fired outside any model borrow; errors go
/// to [`Model::errors`].
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

/// A Lua number-ish → f32 (nil/other → 0.0), for the color-form arguments.
fn num_f32(v: &Value) -> f32 {
    match v {
        Value::Number(n) => *n as f32,
        Value::Integer(i) => *i as f32,
        _ => 0.0,
    }
}

// ── Thumb geometry + drag (decision 0250 §4/§5) ──────────────────────────────────────────────

/// A Slider's thumb rect: the thumb of size `thumb_size` placed at `fraction` along the slider `r`'s
/// track and centered on the cross-axis. The thumb stays **inset** within the track (travel =
/// trackLen − thumbLen), the way WoW scrollbar knobs visibly do. VERTICAL runs y-up-inverted —
/// `fraction` 0 (value=min) sits the thumb flush at the **top** (a scrollbar at 0 is scrolled to the
/// top), `fraction` 1 flush at the bottom. This is the single geometry source shared by `extract`
/// (render) and [`begin_drag`] (hit-test), so render and input never disagree. Returns a [`Rect`] as
/// `(bottom, left, top, right)`.
///
/// `thumb_size` is [`thumb_extent`]'s — the thumb region's OWN size getters, never its authored
/// `<Size>` alone.
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

/// The thumb's extent along both axes — **the thumb region's own `GetWidth`/`GetHeight`**, which
/// is what the client asks for and is not the same thing as its authored `<Size>`.
///
/// `CSimpleSlider`'s pixel→value law reads the thumb through its geometry vtable
/// (`0x789ba0`: `ff 50 1c call [eax+0x1c]` for the horizontal branch, `+0x20` for the vertical), and
/// on a `CSimpleTexture` those slots are `0x770720`/`0x770790` — the **native-texel fallback**:
/// authored span when it is non-zero on that axis, else the art's own texel span through the same
/// `<AbsDimension>` converter, else `0.0` when there is no art at all (ours is
/// [`super::region::virtual_span`], decision 1349).
///
/// Reading `RegionData::size` instead is what broke every Lua-built slider: `SetThumbTexture(path)`
/// authors no size, all four stock `<ThumbTexture>`s declare one, and the old fallback — *the thumb
/// fills the track* — is not a client behaviour at all. It smeared the knob over the whole bar and
/// left `trackLen − thumbLen == 0`, i.e. a slider that cannot move (Dewdrop-2.0's popout, B-report).
/// A thumb with **no region at all** is `None`: `0x789ba0` gates the value math on `+0x328` being
/// non-null, so a thumbless slider takes the press and the capture but never warps.
fn thumb_extent(model: &Model, thumb: Option<crate::widget::RegionHandle>) -> Option<(f32, f32)> {
    Some(super::region::virtual_span(model, thumb?))
}

/// The in-flight thumb drag (decision 0250 §5): the slider being dragged + the grab offset
/// [`slider_grab`] returned, so the thumb tracks the cursor without jumping. Engine C++-equivalent
/// — no Lua, like the real client's scrollbar.
#[derive(Clone, Copy)]
pub(crate) struct SliderDrag {
    pub(crate) slider: FrameHandle,
    /// The grab, in **distance from the track's leading edge** — the axis-neutral frame
    /// [`slider_grab`]/[`slider_fraction`] are stated in, so this arena's y-up rects and the glue
    /// screens' y-down nodes run the identical arithmetic.
    pub(crate) grab_offset: f32,
}

/// On a LeftButton press at `(x, y)` whose hit frame is `hit`: if that frame is a Slider with a
/// resolved rect, begin a drag capture (records [`Model::slider_drag`]). Where the press grabs is
/// [`slider_grab`]'s to say — one law, shared with the glue lane's scrollbars, so an in-game bar
/// and a character-screen bar cannot drift apart on feel.
///
/// There is no enabled gate: 1.12 gives a Slider no enabled state to gate on (see `install`'s note
/// where `Enable`/`Disable`/`IsEnabled` used to be). A slider that should not take the press is
/// kept off it the way the reference keeps anything off it — `EnableMouse(false)`, upstream of here
/// in the hit test.
///
/// Returns `Some((frame id, new value))` when the press itself changed the value (the track jump)
/// — the caller fires `OnValueChanged` outside the model borrow, exactly like [`drag_move`]'s
/// changes — else `None`.
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
    // No thumb region → `0x789ba0`'s `+0x328` gate: the press still captures (the dispatcher's
    // `mov [ebx+0x80],esi` is unconditional), the value never moves.
    let Some(size) = thumb_extent(model, thumb) else {
        model.slider_drag = Some(SliderDrag {
            slider: h,
            grab_offset: 0.0,
        });
        return None;
    };
    let trect = thumb_rect(r, size, vertical, fraction);
    let on_thumb = point_in_rect(trect, x, y);
    // Everything below is stated as distance from the TRACK'S LEADING EDGE — the end the thumb
    // sits at when the value is `min`. Vertical tracks run downward from `r.top` in this y-up
    // arena, so that distance is `top − y`; horizontal ones run rightward from `r.left`.
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

/// On a pointer move at `(x, y)` while a thumb is captured: recompute the value from the cursor
/// position via [`slider_fraction`] (absolute, drift-free — the grab offset keeps the same thumb
/// point under the cursor) and store it. Returns `Some((frame id, new value))` if the value
/// actually changed — the caller fires `OnValueChanged` outside the model borrow — else `None` (no
/// capture, no travel, or no change).
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
    // The same leading-edge frame [`begin_drag`] stored the grab in.
    let (cursor, track_extent, thumb_len) = if vertical {
        (r.top - y, r.height(), th)
    } else {
        (x - r.left, r.width(), tw)
    };
    // `None` = nothing to scroll (a thumb as long as its track).
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
