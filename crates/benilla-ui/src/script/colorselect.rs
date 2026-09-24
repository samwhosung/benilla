//! The `ColorSelect` method surface — the `CSimpleColorSelect` widget behavior over the kind tag
//! (factory `0x6eef90`; LoadXML `0x78b3f0`, script-map `0x78b4f0`). The widget holds one colour and
//! fires `OnColorSelect` (its own
//! script slot, `+0x338`) when that colour is set.
//!
//! **Why this is engine-side and not four lines of Lua on `ColorPickerFrame`.** The corpus creates
//! its own: `TipBuddy.xml` declares two `<ColorSelect>` frames (`TBColorPickerFrame`,
//! `TBColorPickerFrame_Text`), each with its own `<OnColorSelect>` handler, and drives them with
//! `SetColorRGB`/`GetColorRGB`. A method table hung on the one frame `assets/ui` ships would leave
//! those two with a widget that has no methods — the "privileged built-in" failure the `.toc`
//! header names. So it lives with `Slider`/`StatusBar`, in the per-kind dispatcher.
//!
//! **The colour law is byte-verified, and it is not what it looks like.** This module was first
//! written against the obvious model — store three RGB bytes, hand them back — which is wrong.
//! Two corrections, both load-bearing:
//!
//! 1. **The state is HSV `f32`, not RGB** — see [`ColorSelectState`] for the members and why.
//! 2. **The round trip is not the identity.** Inbound and outbound use *different* quantizers
//!    (round-half-up vs floor), so `SetColorRGB` → `GetColorRGB` returns a channel one step low on
//!    **9.7527 % of all 256³ reachable colours** — and, because the map is idempotent-free, the
//!    FrameXML idiom `r,g,b = f:GetColorRGB() … f:SetColorRGB(r,g,b)` **ratchets** a channel
//!    downward one step per cycle. `SetColorRGB(0, 1, 1)` fires `OnColorSelect(0, 254/255, 1)`.
//!    Every Ace2/Dewdrop colour option is that idiom, once per open-and-accept, so this is visible
//!    behaviour, not a curiosity — it is transcribed because it is what the client computes, and
//!    [`ColorSelectState`]'s docs record the one-line change that would undo it if it ever must go.
//!
//! **`SetColorRGB` fires `OnColorSelect`, synchronously, on every call.** VERIFIED at the bytes:
//! `0x78ed01 call 0x78bae0` is a plain fall-through of the last basic block, and inside it the only
//! conditional (`0x78bafd`) tests whether a handler is *bound*. There is deliberately **no
//! change-gate** — the contrast with [`crate::widget::SliderState::store_value`] is real and was
//! verified the other way in the same pass (`CSimpleSlider::SetValue 0x789930` skips on equal at
//! `0x789a16`): two sibling widgets in one band, opposite gating. It matters here because a caller
//! re-setting the colour it already holds still expects its `func` to run — which is exactly how a
//! Dewdrop colour row opens, and how the reference's own `ColorPickerFrame.xml` paints its preview
//! swatch (nothing else ever writes it).
//!
//! **The four texture accessors are carried** (`SetColorWheelTexture` `0x78de90` /
//! `SetColorWheelThumbTexture` `0x78e160` / `SetColorValueTexture` `0x78e450` /
//! `SetColorValueThumbTexture` `0x78e720`, plus the getters `0x78dd80`/`0x78e070`). They had zero
//! callers in the 218-addon corpus and waited for one under decision 1195; the customer that
//! arrived is our own `ColorPickerFrame.xml`, whose four elements the XML loader installs through
//! exactly these — two of them with **no file at all**, because the disc and the strip are pixels
//! the client computes and the app renderer now computes too (the wheel `0x78b580`
//! and the value strip `0x78b8a0` generate them).
//!
//! **NOT carried, still waiting for a customer:** `SetColorHSV`/`GetColorHSV`
//! (`0x78e920`/`0x78ea00`) — zero corpus callers, though the state they would read and write is
//! now the right shape for them, and [`ColorSelectState::set_hsv`] is already the store they'd
//! use (three *raw* `f32` — no clamp, no quantize, zero `fcom` in its body), because the drag path
//! needs exactly that.

use mlua::{Lua, Table, Value};

use super::object::{draw_layer_from_str, frame_handle_of};
use super::pointer::point_in_rect;
use super::region::region_wrapper;
use super::{event, Model, RegionData};
use crate::layout::Rect;
use crate::order::DrawLayer;
use crate::widget::{ColorSelectState, FrameHandle, KindState, RegionHandle, RegionKind};

/// Registry key of the ColorSelect method table (the MAXCSTACK discipline: Lua-side root, named
/// key).
pub(super) const REG_COLORSELECT_METHODS: &str = "__benilla_colorselect_methods";

/// Run `f` over a frame's ColorSelect state under one short write borrow. Errors if `this` is not a
/// live ColorSelect (unreachable through the kind dispatcher, but the method table is a plain Lua
/// value — a caller could fish it out and misapply it).
fn with_colorselect<T>(
    lua: &Lua,
    this: &Table,
    f: impl FnOnce(&mut ColorSelectState) -> T,
) -> mlua::Result<T> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let frame = model
        .arena
        .frame_mut(h)
        .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
    match &mut frame.kind_state {
        KindState::ColorSelect(s) => Ok(f(s)),
        _ => Err(mlua::Error::runtime("not a ColorSelect")),
    }
}

/// One of the widget's four texture sub-objects. The wheel and the strip are the two the press
/// handler (`0x78bf10`) hit-tests (`[+0x318]+0x24` and `[+0x320]+0x24`);
/// the two thumbs are pure output, positioned from the HSV at extract.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Slot {
    Wheel,
    WheelThumb,
    ValueStrip,
    ValueThumb,
}

impl Slot {
    fn get(self, s: &ColorSelectState) -> Option<RegionHandle> {
        match self {
            Slot::Wheel => s.wheel,
            Slot::WheelThumb => s.wheel_thumb,
            Slot::ValueStrip => s.value_strip,
            Slot::ValueThumb => s.value_thumb,
        }
    }

    fn set(self, s: &mut ColorSelectState, rh: RegionHandle) {
        let slot = match self {
            Slot::Wheel => &mut s.wheel,
            Slot::WheelThumb => &mut s.wheel_thumb,
            Slot::ValueStrip => &mut s.value_strip,
            Slot::ValueThumb => &mut s.value_thumb,
        };
        *slot = Some(rh);
    }

    /// The layer a slot's region is born on: the wheel is bound at layer 2 (ARTWORK) through
    /// `0x77fd10`, with the two markers on OVERLAY above the art they ride.
    fn layer(self) -> DrawLayer {
        match self {
            Slot::Wheel | Slot::ValueStrip => DrawLayer::Artwork,
            Slot::WheelThumb | Slot::ValueThumb => DrawLayer::Overlay,
        }
    }
}

/// Get-or-create one slot's texture region; `layer` re-layers an existing one. Returns the region
/// id (for wrapper lookup). The Slider's `ensure_thumb` is the same function one slot wider.
fn ensure_slot(lua: &Lua, this: &Table, slot: Slot, layer: Option<DrawLayer>) -> mlua::Result<u32> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");

    let existing = match &model
        .arena
        .frame(h)
        .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?
        .kind_state
    {
        KindState::ColorSelect(s) => slot.get(s),
        _ => return Err(mlua::Error::runtime("not a ColorSelect")),
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
                .create_region(h, RegionKind::Texture, layer.unwrap_or(slot.layer()), 0)
                .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
            model.region_data.insert(rh, RegionData::default());
            model.touch_layout(); // a region entered the layout gate's read set (decision 0740)
            if let Some(frame) = model.arena.frame_mut(h) {
                if let KindState::ColorSelect(s) = &mut frame.kind_state {
                    slot.set(s, rh);
                }
            }
            rh
        }
    };
    Ok(model.region_id(rh))
}

/// `Set<Slot>Texture([path [, drawLayer]] | r, g, b [, a])`, the Slider's `SetThumbTexture` two-form
/// plus a **third**: called with nothing at all it just creates the region. That empty form is not a
/// convenience — it is the only one the two file-less elements can use, and it is what
/// `<ColorWheelTexture/>` means. A slot left file-less is where the app renderer paints.
fn install_slot_texture(
    lua: &Lua,
    m: &Table,
    slot: Slot,
    setter: &str,
    getter: &str,
) -> mlua::Result<()> {
    m.set(
        setter,
        lua.create_function(
            move |lua, (this, a1, a2, a3, a4): (Table, Value, Value, Value, Value)| {
                let layer = match &a2 {
                    Value::String(s) => s.to_str().ok().and_then(|l| draw_layer_from_str(&l)),
                    _ => None,
                };
                let id = ensure_slot(lua, &this, slot, layer)?;
                let rh = {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    *model.id_to_region.get(&id).expect("colorselect region id")
                };
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                let data = model.region_data.entry(rh).or_default();
                match &a1 {
                    Value::String(s) => {
                        data.texture = Some(s.to_str()?.to_string());
                        data.fill = None;
                    }
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
                    // The empty form: the region now exists and stays file-less.
                    _ => {}
                }
                Ok(())
            },
        )?,
    )?;
    m.set(
        getter,
        lua.create_function(move |lua, this: Table| {
            let existing = with_colorselect(lua, &this, |s| slot.get(s))?;
            let id = {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                existing.map(|rh| model.region_id(rh))
            };
            match id {
                Some(id) => Ok(Value::Table(region_wrapper(lua, id)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;
    Ok(())
}

/// A Lua number argument as `f32`, 0 for anything else — the Slider's own colour-form helper.
fn num_f32(v: &Value) -> f32 {
    match v {
        Value::Number(n) => *n as f32,
        Value::Integer(i) => *i as f32,
        _ => 0.0,
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    m.set(
        "SetColorRGB",
        // Shape C on r, g, b (`ColorSelect:SetColorRGB 0x78eae0`, `2=C 3=C 4=C 5=B`): bare
        // `lua_tonumber`, nil → 0.0, no raise.
        lua.create_function(|lua, (this, r, g, b): (Table, Value, Value, Value)| {
            let (r, g, b) = (
                crate::script::object::as_f64(&r),
                crate::script::object::as_f64(&g),
                crate::script::object::as_f64(&b),
            );
            // Store through the client's quantize, then fire with what the widget now *holds* —
            // the round-tripped values, identical to the next GetColorRGB, not the raw arguments.
            let (qr, qg, qb) = with_colorselect(lua, &this, |s| {
                s.set_rgb(r, g, b);
                s.rgb_f64()
            })?;
            fire_color_select(lua, &this, qr, qg, qb)
        })?,
    )?;
    m.set(
        "GetColorRGB",
        lua.create_function(|lua, this: Table| with_colorselect(lua, &this, |s| s.rgb_f64()))?,
    )?;

    // The four texture accessors (`0x78de90`/`0x78e160`/`0x78e450`/`0x78e720` and the getters
    // `0x78dd80`/`0x78e070`). They had zero corpus callers and waited for one; the
    // customer that arrived is our own `ColorPickerFrame.xml`, whose four elements the XML loader
    // installs through exactly these.
    install_slot_texture(
        lua,
        &m,
        Slot::Wheel,
        "SetColorWheelTexture",
        "GetColorWheelTexture",
    )?;
    install_slot_texture(
        lua,
        &m,
        Slot::WheelThumb,
        "SetColorWheelThumbTexture",
        "GetColorWheelThumbTexture",
    )?;
    install_slot_texture(
        lua,
        &m,
        Slot::ValueStrip,
        "SetColorValueTexture",
        "GetColorValueTexture",
    )?;
    install_slot_texture(
        lua,
        &m,
        Slot::ValueThumb,
        "SetColorValueThumbTexture",
        "GetColorValueThumbTexture",
    )?;

    lua.set_named_registry_value(REG_COLORSELECT_METHODS, m)?;
    Ok(())
}

/// Fire `OnColorSelect(self, r, g, b)` — the widget's own script slot (`+0x338`). Fired
/// outside the model borrow; a handler error goes to [`Model::errors`] rather than back to the
/// setter's caller, the same posture as the Slider's `OnValueChanged`.
fn fire_color_select(lua: &Lua, this: &Table, r: f64, g: f64, b: f64) -> mlua::Result<()> {
    let id = {
        let h = frame_handle_of(lua, this)?;
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        model.frame_id(h)
    };
    if let Err(e) = event::fire_widget_handler(
        lua,
        id,
        "OnColorSelect",
        vec![Value::Number(r), Value::Number(g), Value::Number(b)],
    ) {
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .record_script_error(e.to_string());
    }
    Ok(())
}

/// The wheel's normalised coordinates for a cursor at `(x, y)`: the offset from the wheel rect's
/// centre, scaled by its **half-extents** — so a non-square wheel normalises to a disc, not an
/// ellipse (`0x78bdd0`..: `nx=(x−cx)/((right−left)·0.5)`, `ny=(y−cy)/((top−bottom)·0.5)`). y is up,
/// this arena's convention and the client's.
fn wheel_norm(r: Rect, x: f32, y: f32) -> (f32, f32) {
    let hw = (r.right - r.left) * 0.5;
    let hh = (r.top - r.bottom) * 0.5;
    let cx = (r.left + r.right) * 0.5;
    let cy = (r.bottom + r.top) * 0.5;
    // A degenerate rect (an unresolved region) would divide by zero; the client cannot reach that
    // state because it only hit-tests a laid-out texture, and neither can we — but 0/0 is NaN, and
    // NaN in the HSV would poison every colour after it.
    let nx = if hw != 0.0 { (x - cx) / hw } else { 0.0 };
    let ny = if hh != 0.0 { (y - cy) / hh } else { 0.0 };
    (nx, ny)
}

/// The hue/saturation a press or drag at `(x, y)` writes, given the wheel's rect. `H` runs from
/// `atan2(ny, nx) + π` in degrees — so the wheel's **left** edge is 0°/360° (red) and its right edge
/// 180° (cyan) — and `S` is the radius, clamped at the rim. Raw f32 the whole way: the drag path
/// never touches a quantizer.
fn wheel_hs(r: Rect, x: f32, y: f32) -> (f32, f32) {
    let (nx, ny) = wheel_norm(r, x, y);
    let radius = (nx * nx + ny * ny).sqrt();
    let hue = (ny.atan2(nx) + std::f32::consts::PI).to_degrees();
    (hue, radius.min(1.0))
}

/// The brightness a press or drag at `y` writes, given the strip's rect: the fraction of the way up
/// it, clamped — `V = clamp((y − bottom) / (top − bottom), 0, 1)` (`0x78beed`). x is ignored, which
/// is why a drag that wanders sideways off the strip keeps working.
fn strip_value(r: Rect, y: f32) -> f32 {
    let span = r.top - r.bottom;
    if span == 0.0 {
        return 0.0;
    }
    ((y - r.bottom) / span).clamp(0.0, 1.0)
}

/// The wheel thumb's rect: the marker seated at the point the current `(H, S)` *came from*. It is
/// the inverse of [`wheel_hs`], which is the whole invariant — click a pixel, the marker lands on
/// that pixel.
///
/// Byte-exact to `0x78bc20`, which does
/// `SetPoint(CENTER, wheel, CENTER, −m·cos θ, −m·sin θ)` with `θ = H·π/180` and
/// `m = GetWidth(wheel)·0.5·S`. Its two `fchs` are the pick law's `+π` seen from the other side:
/// drop either sign and the marker sits diametrically opposite the colour it marks.
///
/// **The width is used on both axes**, deliberately — the client reads `GetWidth` for the vertical
/// offset too, while the pick law divides `y` by half-*height*. On a non-square wheel the two
/// therefore disagree, and that asymmetry is reproduced rather than fixed. The reference's wheel is
/// square, so it costs nothing there and would cost fidelity anywhere else.
///
/// `S == 0` puts it dead centre whatever the hue is, which is what makes the `-1` grey sentinel
/// (`ColorSelectState::hsv`) harmless here: it is multiplied by a zero radius.
pub(super) fn wheel_thumb_rect(wheel: Rect, thumb_size: Option<(f32, f32)>, hsv: [f32; 3]) -> Rect {
    let (tw, th) = thumb_size.unwrap_or((0.0, 0.0));
    let cx = (wheel.left + wheel.right) * 0.5;
    let cy = (wheel.bottom + wheel.top) * 0.5;
    let m = (wheel.right - wheel.left) * 0.5 * hsv[1];
    let theta = hsv[0].to_radians();
    let (x, y) = (cx - m * theta.cos(), cy - m * theta.sin());
    Rect::new(y - th * 0.5, x - tw * 0.5, y + th * 0.5, x + tw * 0.5)
}

/// The value thumb's rect: centred on the strip horizontally (the reference's marker is 48 wide
/// over a 32-wide strip, so it deliberately overhangs) and seated `V` of the way up from its
/// BOTTOM — `0x78bcf0`'s `SetPoint(CENTER, strip, BOTTOM, 0, V·GetHeight)`.
///
/// **The height it scales by is the WHEEL's, not the strip's** — `0x78bcf0` dereferences
/// `[this+0x318]`, the wheel, with no null guard of its own, while anchoring to the strip. The two
/// are both 128 tall in the reference, so it is invisible there; it is reproduced because a client
/// that "fixed" it would place the marker somewhere the real one does not the moment an addon
/// declares a strip of its own height. A widget with no wheel falls back to the strip, which is the
/// nearest thing to the client's unguarded read that does not put a NaN in a rect.
pub(super) fn value_thumb_rect(
    strip: Rect,
    wheel: Option<Rect>,
    thumb_size: Option<(f32, f32)>,
    hsv: [f32; 3],
) -> Rect {
    let (tw, th) = thumb_size.unwrap_or((0.0, 0.0));
    let cx = (strip.left + strip.right) * 0.5;
    let scale_height = wheel.map_or(strip.top - strip.bottom, |w| w.top - w.bottom);
    let y = strip.bottom + hsv[2].clamp(0.0, 1.0) * scale_height;
    Rect::new(y - th * 0.5, cx - tw * 0.5, y + th * 0.5, cx + tw * 0.5)
}

/// The in-flight colour drag. The client keeps **two independent flags** (`+0x314` wheel, `+0x315`
/// strip), both set by the same press if the rects overlap at the cursor, and its cursor-position
/// handler applies whichever are set — so this is a pair of bools, not an enum. It also explains the
/// feel: a drag that starts on the wheel keeps writing hue and saturation no matter where the cursor
/// wanders, because the flag, not the current position, decides.
#[derive(Clone, Copy)]
pub(crate) struct ColorDrag {
    pub(crate) frame: FrameHandle,
    wheel: bool,
    strip: bool,
}

/// The two hit rects of a ColorSelect frame — its wheel region's and its value strip's, as resolved
/// by layout. `None` for a slot with no region, which is every ColorSelect that never declared one.
fn hit_rects(model: &Model, h: FrameHandle) -> (Option<Rect>, Option<Rect>) {
    let (wheel, strip) = match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(KindState::ColorSelect(s)) => (s.wheel, s.value_strip),
        _ => (None, None),
    };
    let rect = |rh: Option<RegionHandle>| rh.and_then(|rh| model.region_resolved.get(&rh).copied());
    (rect(wheel), rect(strip))
}

/// On a LeftButton press at `(x, y)` whose hit frame is `hit`: if that frame is a ColorSelect and
/// the cursor is inside its wheel or its value strip, capture the drag and **apply it immediately**
/// — `0x78bf10` calls the cursor-position handler through `[eax+0x3c]` before it returns, so a
/// single click jumps the colour to the click point and fires. Returns what the caller must fire.
pub(super) fn begin_drag(
    model: &mut Model,
    hit: Option<FrameHandle>,
    x: f32,
    y: f32,
) -> Option<(u32, f64, f64, f64)> {
    let h = hit?;
    let (wheel, strip) = hit_rects(model, h);
    let on_wheel = wheel.is_some_and(|r| point_in_rect(r, x, y));
    let on_strip = strip.is_some_and(|r| point_in_rect(r, x, y));
    if !on_wheel && !on_strip {
        return None;
    }
    model.color_drag = Some(ColorDrag {
        frame: h,
        wheel: on_wheel,
        strip: on_strip,
    });
    drag_move(model, x, y)
}

/// On a pointer move at `(x, y)` while a colour drag is captured: rewrite whichever of H/S and V the
/// capture owns, straight into the widget's HSV floats, and hand the caller the frame id plus the
/// colour to fire `OnColorSelect` with. Returns `Some` on **every** captured move, with no
/// change-gate — the widget deliberately has none (`0x78bafd` tests only whether a handler is
/// bound), which is the opposite of the Slider next door.
pub(super) fn drag_move(model: &mut Model, x: f32, y: f32) -> Option<(u32, f64, f64, f64)> {
    let ColorDrag {
        frame,
        wheel,
        strip,
    } = *model.color_drag.as_ref()?;
    let (wheel_rect, strip_rect) = hit_rects(model, frame);
    let hs = if wheel {
        wheel_rect.map(|r| wheel_hs(r, x, y))
    } else {
        None
    };
    let v = if strip {
        strip_rect.map(|r| strip_value(r, y))
    } else {
        None
    };
    if hs.is_none() && v.is_none() {
        return None;
    }
    let rgb = match model.arena.frame_mut(frame).map(|f| &mut f.kind_state) {
        Some(KindState::ColorSelect(s)) => {
            let mut hsv = s.hsv;
            if let Some((hue, sat)) = hs {
                hsv[0] = hue;
                hsv[1] = sat;
            }
            if let Some(v) = v {
                hsv[2] = v;
            }
            s.set_hsv(hsv[0], hsv[1], hsv[2]);
            s.rgb_f64()
        }
        _ => return None, // the frame died mid-drag
    };
    Some((model.frame_id(frame), rgb.0, rgb.1, rgb.2))
}

/// Release any in-flight colour drag (LeftButton up, or the pointer leaving the window) — the
/// widget's `0x78bf90`, which clears both flags.
pub(super) fn end_drag(model: &mut Model) {
    model.color_drag = None;
}

/// Fire `OnColorSelect` for a frame the pointer path just recoloured. The setter's own
/// [`fire_color_select`] needs the caller's `this` table; the pointer has only an id, so this is the
/// same two lines against [`event::fire_widget_handler`] directly.
pub(super) fn fire_by_id(lua: &Lua, id: u32, r: f64, g: f64, b: f64) -> mlua::Result<()> {
    event::fire_widget_handler(
        lua,
        id,
        "OnColorSelect",
        vec![Value::Number(r), Value::Number(g), Value::Number(b)],
    )
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;
    use crate::widget::ColorSelectState;

    /// A `ColorSelect` starts **white** (the ctor's `H=0, S=0, V=1`), not black. A zero-init store
    /// would have made it black, which is the tell that the state is HSV and not RGB.
    #[test]
    fn a_fresh_color_select_is_white() {
        let s = UiScript::new().unwrap();
        s.run(r#"cs = CreateFrame("ColorSelect", "TestColorSelectNew")"#)
            .unwrap();
        let (r, g, b): (f64, f64, f64) = s.eval("return cs:GetColorRGB()").unwrap();
        assert_eq!((r, g, b), (1.0, 1.0, 1.0));
    }

    /// **The round trip is not the identity, and the deviation is the client's.** `(0, 1, 1)` — cyan,
    /// a two-channel tie at the maximum — comes back with green one step low, exactly as the
    /// reference does. Greys are its control class and never lose.
    #[test]
    fn the_round_trip_loses_exactly_one_step_on_the_witness_colour() {
        let s = UiScript::new().unwrap();
        s.run(r#"cs = CreateFrame("ColorSelect", "TestColorSelectRT")"#)
            .unwrap();
        s.run("cs:SetColorRGB(0, 1, 1)").unwrap();
        let (r, g, b): (f64, f64, f64) = s.eval("return cs:GetColorRGB()").unwrap();
        assert_eq!(r, 0.0);
        assert_eq!(g, 254.0 / 255.0, "the tied maximum loses one step");
        assert_eq!(b, 1.0);

        // The control: a grey survives exactly (the `S == 0` leg copies V and never reads hue).
        for byte in [0u8, 1, 64, 128, 254, 255] {
            let v = f64::from(byte) / 255.0;
            s.run(&format!("cs:SetColorRGB({v}, {v}, {v})")).unwrap();
            let (gr, gg, gb): (f64, f64, f64) = s.eval("return cs:GetColorRGB()").unwrap();
            assert_eq!((gr, gg, gb), (v, v, v), "grey {byte} must survive exactly");
        }
    }

    /// **And it ratchets.** The FrameXML idiom — read the colour back, hand it straight to
    /// `SetColorRGB` — re-applies the same lossy map instead of settling. `(0, 8, 132)`
    /// walks to `(0, 0, 132)` in eight cycles. Every Ace2/Dewdrop colour option is that idiom, once
    /// per open-and-accept, so this is the shape of a real player-visible drift and it is deliberate.
    #[test]
    fn the_read_back_ratchets_a_channel_downward() {
        let s = UiScript::new().unwrap();
        s.run(r#"cs = CreateFrame("ColorSelect", "TestColorSelectRatchet")"#)
            .unwrap();
        s.run("cs:SetColorRGB(0/255, 8/255, 132/255)").unwrap();
        let mut seen = Vec::new();
        for _ in 0..8 {
            let g: f64 = s
                .eval("local r, g, b = cs:GetColorRGB() cs:SetColorRGB(r, g, b) return g")
                .unwrap();
            seen.push((g * 255.0).round() as u8);
        }
        assert_eq!(
            seen,
            vec![7, 6, 5, 4, 3, 2, 1, 0],
            "one step down per cycle, to a fixed point at 0"
        );
        // The fixed point holds — it is a ratchet, not a runaway.
        let g: f64 = s
            .eval("local r, g, b = cs:GetColorRGB() cs:SetColorRGB(r, g, b) return g")
            .unwrap();
        assert_eq!(g, 0.0);
    }

    /// The two quantizers, directly. A is round-half-up and clamps; B is a floor with no clamp.
    #[test]
    fn the_two_quantizers_disagree_in_the_verified_way() {
        // A — inbound. The clamp is part of it, so `2` is white and `-1` is black, not a wrapped byte.
        assert_eq!(ColorSelectState::quantize_a(-1.0), 0);
        assert_eq!(ColorSelectState::quantize_a(0.0), 0);
        assert_eq!(ColorSelectState::quantize_a(1.0), 255);
        assert_eq!(ColorSelectState::quantize_a(2.0), 255);
        // The half-up bias: 0.5·255 = 127.5, +0.5 = 128.0, truncated = 128.
        assert_eq!(ColorSelectState::quantize_a(0.5), 128);

        // B — outbound. Endpoints agree with A; a value a hair below a boundary does NOT.
        assert_eq!(ColorSelectState::quantize_b(0.0), 0);
        assert_eq!(ColorSelectState::quantize_b(1.0), 255);
        assert_eq!(ColorSelectState::quantize_b(0.5), 127, "floor, not half-up");
        assert_ne!(
            ColorSelectState::quantize_b(0.5),
            ColorSelectState::quantize_a(0.5),
            "the disagreement IS the mechanism behind the drift"
        );
        // No clamp on B — out of range wraps mod 256 (reachable only through SetColorHSV).
        assert_eq!(ColorSelectState::quantize_b(-0.001), 255);
    }

    /// `SetColorRGB` fires `OnColorSelect` with the colour the widget now holds, and fires **again**
    /// when the same colour is set — no change-gate (the module doc's byte-verified reason).
    #[test]
    fn set_color_rgb_fires_on_color_select_every_time() {
        let s = UiScript::new().unwrap();
        s.run(
            r#"
            fired = {}
            cs = CreateFrame("ColorSelect", "TestColorSelect2")
            cs:SetScript("OnColorSelect", function()
                table.insert(fired, arg1 .. "," .. arg2 .. "," .. arg3)
            end)
        "#,
        )
        .unwrap();
        s.run("cs:SetColorRGB(1, 0, 0)").unwrap();
        s.run("cs:SetColorRGB(1, 0, 0)").unwrap();
        assert!(s.errors().is_empty(), "{:?}", s.errors());
        assert_eq!(s.eval::<usize>("return table.getn(fired)").unwrap(), 2);
        assert_eq!(s.eval::<String>("return fired[1]").unwrap(), "1,0,0");

        // The handler's arguments are bit-identical to a GetColorRGB() on the next line — the
        // binary reaches them through literally the same two calls, and so do we.
        s.run("fired = {} cs:SetColorRGB(0, 1, 1)").unwrap();
        let (from_handler, from_getter): (String, String) = s
            .eval("local r, g, b = cs:GetColorRGB() return fired[1], r .. \",\" .. g .. \",\" .. b")
            .unwrap();
        assert_eq!(from_handler, from_getter);
    }

    /// The methods are the ColorSelect's alone — a plain frame duck-types as *not* one, which is how
    /// `if frame.SetColorRGB then` reads in an addon.
    #[test]
    fn the_methods_do_not_leak_onto_other_kinds() {
        let s = UiScript::new().unwrap();
        s.run(r#"plain = CreateFrame("Frame", "TestPlainFrame")"#)
            .unwrap();
        assert!(s.eval::<bool>("return plain.SetColorRGB == nil").unwrap());
        assert!(s.eval::<bool>("return plain.GetColorRGB == nil").unwrap());
    }
    // ─────────────────────────────────────────────────────────────────────────────────────────
    // The wheel and the strip — the pick, the drag, and the two markers
    // ─────────────────────────────────────────────────────────────────────────────────────────

    /// A `ColorSelect` with the reference's own geometry, laid out and ready to click: a 128×128
    /// wheel at the frame's top-left and a 32×128 strip to its right. Built through the XML loader
    /// so the four elements' whole install path — `apply_colorselect`, the file-less setter form,
    /// the getter round-trip that carries `<Size>`/`<Anchors>` — is what these tests exercise.
    fn picker() -> UiScript {
        let mut s = UiScript::new().unwrap();
        let xml = r#"<Ui>
            <ColorSelect name="TestPicker" parent="UIParent" enableMouse="true">
                <Size><AbsDimension x="365" y="200"/></Size>
                <Anchors><Anchor point="BOTTOMLEFT"/></Anchors>
                <ColorWheelTexture name="TestPickerWheel">
                    <Size><AbsDimension x="128" y="128"/></Size>
                    <Anchors><Anchor point="TOPLEFT"><Offset><AbsDimension x="16" y="-32"/></Offset></Anchor></Anchors>
                </ColorWheelTexture>
                <ColorWheelThumbTexture file="Interface\Buttons\UI-ColorPicker-Buttons">
                    <Size><AbsDimension x="10" y="10"/></Size>
                </ColorWheelThumbTexture>
                <ColorValueTexture>
                    <Size><AbsDimension x="32" y="128"/></Size>
                    <Anchors><Anchor point="LEFT" relativeTo="TestPickerWheel" relativePoint="RIGHT"><Offset><AbsDimension x="24" y="0"/></Offset></Anchor></Anchors>
                </ColorValueTexture>
                <ColorValueThumbTexture file="Interface\Buttons\UI-ColorPicker-Buttons">
                    <Size><AbsDimension x="48" y="14"/></Size>
                </ColorValueThumbTexture>
            </ColorSelect>
        </Ui>"#;
        let doc = crate::framexml::parse(xml).unwrap();
        let report = crate::loader::load(&s, &doc, &|_| None);
        assert!(
            report.errors.is_empty(),
            "loader errors: {:?}",
            report.errors
        );
        s.set_screen_size(1024.0, 768.0);
        s.resolve();
        s
    }

    /// The wheel's rect, from the region the loader published as a global.
    fn wheel_rect(s: &mut UiScript) -> (f32, f32, f32, f32) {
        let (l, b, w, h): (f32, f32, f32, f32) = s
            .eval(
                "local r = TestPickerWheel; \
                 return r:GetLeft(), r:GetBottom(), r:GetWidth(), r:GetHeight()",
            )
            .unwrap();
        (l, b, w, h)
    }

    /// The four elements install as real regions with the authored geometry, and the wheel's name
    /// publishes as a global — which is what `<ColorValueTexture>`'s own anchor resolves against.
    #[test]
    fn the_four_elements_install_with_their_authored_geometry() {
        let mut s = picker();
        let (l, b, w, h) = wheel_rect(&mut s);
        assert_eq!((w, h), (128.0, 128.0), "the wheel is 128 square");
        // TOPLEFT (16, −32) inside a 365×200 frame seated at the screen's bottom-left.
        assert_eq!(l, 16.0);
        assert_eq!(b, 200.0 - 32.0 - 128.0);
        let (sl, sw, sh): (f32, f32, f32) = s
            .eval(
                "local r = TestPicker:GetColorValueTexture(); \
                 return r:GetLeft(), r:GetWidth(), r:GetHeight()",
            )
            .unwrap();
        assert_eq!((sw, sh), (32.0, 128.0), "the strip is 32×128");
        assert_eq!(sl, 16.0 + 128.0 + 24.0, "seated 24 right of the wheel");
        // The thumbs exist and carry the one BLP in the window.
        let file: String = s
            .eval("return TestPicker:GetColorWheelThumbTexture():GetTexture()")
            .unwrap();
        assert!(file.contains("UI-ColorPicker-Buttons"), "got {file}");
    }

    /// **A click on the wheel picks the colour that pixel shows.** The disc's law and the pick law
    /// are inverses (`0x78b580` draws it, `0x78bdd0` picks it), so
    /// this is stated at the four cardinal points, where the hue is nameable: LEFT is red, RIGHT
    /// cyan, TOP violet, BOTTOM chartreuse. A sign error anywhere in the chain mirrors one axis and
    /// two of these four flip.
    #[test]
    fn a_click_on_the_wheel_picks_the_hue_under_the_cursor() {
        let mut s = picker();
        let (l, b, w, h) = wheel_rect(&mut s);
        let (cx, cy) = (l + w * 0.5, b + h * 0.5);
        // One pixel inside each rim, so saturation is ~1 and the hue is pure.
        for (name, x, y, want) in [
            ("left", l + 1.0, cy, [1.0, 0.0, 0.0]),
            ("right", l + w - 1.0, cy, [0.0, 1.0, 1.0]),
            ("top", cx, b + h - 1.0, [0.5, 0.0, 1.0]),
            ("bottom", cx, b + 1.0, [0.5, 1.0, 0.0]),
        ] {
            s.mouse_button(x, y, "LeftButton", true);
            s.mouse_button(x, y, "LeftButton", false);
            let (r, g, bl): (f64, f64, f64) = s.eval("return TestPicker:GetColorRGB()").unwrap();
            for (ch, (got, wanted)) in [r, g, bl].iter().zip(want).enumerate() {
                assert!(
                    (got - wanted).abs() < 0.06,
                    "{name} rim: channel {ch} is {got:.3}, expected ~{wanted}"
                );
            }
        }
    }

    /// The centre is saturation 0 — white — and it stays white however the hue would have come out,
    /// because the `-1` grey sentinel is inert on the way back through `hsv_to_rgb`.
    #[test]
    fn the_wheels_centre_is_unsaturated() {
        let mut s = picker();
        let (l, b, w, h) = wheel_rect(&mut s);
        s.mouse_button(l + w * 0.5, b + h * 0.5, "LeftButton", true);
        let (r, g, bl): (f64, f64, f64) = s.eval("return TestPicker:GetColorRGB()").unwrap();
        assert!(
            r > 0.99 && g > 0.99 && bl > 0.99,
            "the centre is white, got ({r:.3}, {g:.3}, {bl:.3})"
        );
    }

    /// **The press IS the first move of the gesture**, and the drag keeps writing the wheel even
    /// after the cursor leaves it — the capture is a flag on the widget, not a hit test per move
    /// (`0x78bd80` is gated on `+0x314`, never re-tested). Dragging from red round to cyan without
    /// lifting proves both.
    #[test]
    fn a_wheel_drag_keeps_the_capture_when_the_cursor_wanders_off() {
        let mut s = picker();
        let (l, b, w, h) = wheel_rect(&mut s);
        let cy = b + h * 0.5;
        s.mouse_button(l + 1.0, cy, "LeftButton", true);
        // Straight across and well past the right rim — outside the wheel's rect entirely.
        s.mouse_move(l + w + 200.0, cy);
        let (r, g, bl): (f64, f64, f64) = s.eval("return TestPicker:GetColorRGB()").unwrap();
        assert!(
            r < 0.05 && g > 0.95 && bl > 0.95,
            "the drag followed the cursor past the rim, got ({r:.3}, {g:.3}, {bl:.3})"
        );
        // Saturation clamps at the rim rather than running past it.
        s.mouse_button(l + w + 200.0, cy, "LeftButton", false);
        // Released: further moves write nothing.
        s.mouse_move(l + w * 0.5, cy);
        let after: (f64, f64, f64) = s.eval("return TestPicker:GetColorRGB()").unwrap();
        assert_eq!(
            after,
            (r, g, bl),
            "a move after the release must not touch the colour"
        );
    }

    /// The strip writes **only** brightness: dragging it down to black and back up leaves the hue
    /// exactly where the wheel put it.
    #[test]
    fn a_strip_drag_writes_brightness_and_leaves_the_hue_alone() {
        let mut s = picker();
        let (l, b, w, h) = wheel_rect(&mut s);
        s.mouse_button(l + 1.0, b + h * 0.5, "LeftButton", true); // red
        s.mouse_button(l + 1.0, b + h * 0.5, "LeftButton", false);
        let (sl, sb, sw, sh): (f32, f32, f32, f32) = s
            .eval(
                "local r = TestPicker:GetColorValueTexture(); \
                 return r:GetLeft(), r:GetBottom(), r:GetWidth(), r:GetHeight()",
            )
            .unwrap();
        let sx = sl + sw * 0.5;
        s.mouse_button(sx, sb + sh * 0.5, "LeftButton", true);
        let (r, g, bl): (f64, f64, f64) = s.eval("return TestPicker:GetColorRGB()").unwrap();
        assert!(
            (r - 0.5).abs() < 0.02 && g < 0.02 && bl < 0.02,
            "half-way up the strip is half-brightness red, got ({r:.3}, {g:.3}, {bl:.3})"
        );
        s.mouse_move(sx, sb - 50.0); // dragged below the strip: clamps to black
        let black: (f64, f64, f64) = s.eval("return TestPicker:GetColorRGB()").unwrap();
        assert_eq!(black, (0.0, 0.0, 0.0), "V clamps at 0");
        // (Black is exact whatever the hue: `hsv_to_rgb` scales every channel by V.)
        s.mouse_move(sx, sb + sh + 50.0); // and above it: clamps to full
        let full: (f64, f64, f64) = s.eval("return TestPicker:GetColorRGB()").unwrap();
        assert!(
            full.0 == 1.0 && full.1 < 0.02 && full.2 < 0.02,
            "V clamps at 1 and the hue is untouched, got {full:?}"
        );
        // `w` and the unused wheel bounds are only here to seat the click above.
        let _ = (w, l);
    }

    /// `OnColorSelect` fires on **every** step of a drag, including one that lands on the colour
    /// the widget already holds — the widget has no change-gate (`0x78bafd` tests only whether a
    /// handler is bound), which is the opposite of the Slider next door.
    #[test]
    fn the_drag_fires_on_every_move_with_no_change_gate() {
        let mut s = picker();
        s.run("fires = 0; TestPicker:SetScript('OnColorSelect', function() fires = fires + 1 end)")
            .unwrap();
        let (l, b, w, h) = wheel_rect(&mut s);
        let (cx, cy) = (l + w * 0.5, b + h * 0.5);
        s.mouse_button(cx, cy, "LeftButton", true);
        let after_press: i64 = s.eval("return fires").unwrap();
        assert_eq!(after_press, 1, "the press itself fires once");
        for _ in 0..3 {
            s.mouse_move(cx, cy); // the SAME point — no change, still fires
        }
        let after_moves: i64 = s.eval("return fires").unwrap();
        assert_eq!(after_moves, 4, "no change-gate: every captured move fires");
    }

    /// A press that lands on neither the wheel nor the strip captures nothing — the surrounding
    /// window is not a giant colour surface.
    #[test]
    fn a_press_outside_both_rects_captures_nothing() {
        let mut s = picker();
        let before: (f64, f64, f64) = s.eval("return TestPicker:GetColorRGB()").unwrap();
        s.mouse_button(340.0, 20.0, "LeftButton", true); // inside the frame, below both
        s.mouse_move(20.0, 150.0); // ... and now over the wheel, uncaptured
        let after: (f64, f64, f64) = s.eval("return TestPicker:GetColorRGB()").unwrap();
        assert_eq!(after, before, "no capture, no colour change");
    }

    /// The two markers sit where the colour came from. Checked through **extract**, not `GetLeft`:
    /// the markers carry no anchors — the client `ClearAllPoints`es them and re-`SetPoint`s from
    /// C++, and benilla derives their rects at extract for the same reason — so the layout resolver
    /// has nothing to answer with. That is the same shape as the Slider thumb's, and it is the one
    /// place a corpus addon could tell the difference; nothing in the 218 reads a picker marker's
    /// rect.
    ///
    /// The wheel marker is checked by round trip — click a point, the marker's centre lands back on
    /// it — which is the invariant that matters and the one either `fchs` breaks.
    #[test]
    fn the_markers_land_where_the_colour_came_from() {
        let mut s = picker();
        let (l, b, w, h) = wheel_rect(&mut s);
        let (cx, cy) = (l + w * 0.5, b + h * 0.5);
        for (dx, dy) in [(40.0, 0.0), (0.0, 40.0), (-30.0, -30.0), (0.0, 0.0)] {
            s.mouse_button(cx + dx, cy + dy, "LeftButton", true);
            s.mouse_button(cx + dx, cy + dy, "LeftButton", false);
            s.resolve();
            let r = marker_rect(&s, 10.0).expect("the wheel marker draws");
            let (mx, my) = ((r.left + r.right) * 0.5, (r.bottom + r.top) * 0.5);
            assert!(
                (mx - (cx + dx)).abs() < 1.0 && (my - (cy + dy)).abs() < 1.0,
                "clicked ({}, {}), marker centred at ({mx}, {my})",
                cx + dx,
                cy + dy
            );
        }
        // The value marker rides V from the strip's bottom — and scales by the WHEEL's height,
        // which is the client's own unguarded `[this+0x318]` read. Both are 128 here.
        let (sb, sh): (f32, f32) = s
            .eval(
                "local r = TestPicker:GetColorValueTexture(); return r:GetBottom(), r:GetHeight()",
            )
            .unwrap();
        for v in [0.0_f32, 0.25, 1.0] {
            s.run(&format!("TestPicker:SetColorRGB({v}, 0, 0)"))
                .unwrap();
            s.resolve();
            let r = marker_rect(&s, 48.0).expect("the value marker draws");
            let my = (r.bottom + r.top) * 0.5;
            assert!(
                (my - (sb + v * sh)).abs() < 1.0,
                "V={v}: marker at {my}, expected {}",
                sb + v * sh
            );
        }
    }

    /// The extracted rect of the marker whose authored width is `width` — the two markers share
    /// one BLP and are told apart by their size (10 for the wheel's, 48 for the strip's), which is
    /// also the assertion that they kept it.
    fn marker_rect(s: &UiScript, width: f32) -> Option<crate::layout::Rect> {
        s.extract().into_iter().find_map(|q| {
            let r = q.rect?;
            let is_marker = matches!(
                &q.content,
                crate::script::QuadContent::Texture { path: Some(p), .. }
                    if p.contains("UI-ColorPicker-Buttons")
            );
            (is_marker && (r.right - r.left - width).abs() < 0.01).then_some(r)
        })
    }
}
