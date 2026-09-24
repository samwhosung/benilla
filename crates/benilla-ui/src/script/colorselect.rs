//! The `ColorSelect` methods: the `CSimpleColorSelect` widget (factory `0x6eef90`, XML load
//! `0x78b3f0`), one colour held as HSV floats. `SetColorRGB` fires `OnColorSelect` (script map
//! `0x78b4f0`, slot `+0x338`) on every call: `0x78ed01` calls `0x78bae0`, whose only test
//! (`0x78bafd`) is for a bound handler, while `CSimpleSlider::SetValue` (`0x789930`) skips an equal
//! value. The RGB round trip loses a step on 9.75 % of colours, as in the reference: in and out use
//! different quantizers (round-half-up, then floor), so reading back and setting ratchets a channel
//! down; see [`ColorSelectState`].

use mlua::{Lua, Table, Value};

use super::object::{draw_layer_from_str, frame_handle_of};
use super::pointer::point_in_rect;
use super::region::region_wrapper;
use super::{event, Model, RegionData};
use crate::layout::Rect;
use crate::order::DrawLayer;
use crate::widget::{ColorSelectState, FrameHandle, KindState, RegionHandle, RegionKind};

/// Registry key of the ColorSelect method table (the MAXCSTACK discipline: a Lua-side root).
pub(super) const REG_COLORSELECT_METHODS: &str = "__benilla_colorselect_methods";

/// Run `f` over a frame's ColorSelect state under one short write borrow; errors if `this` is not
/// a live ColorSelect, as the method table is a plain Lua value a caller can misapply.
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

/// One of the widget's four texture sub-objects: the press handler (`0x78bf10`) hit-tests the
/// wheel (`+0x318`) and the strip (`+0x320`); the thumbs are output only, placed at extract.
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

    /// The layer a slot's region is born on: ARTWORK for the art (the wheel's layer 2, through
    /// `0x77fd10`), OVERLAY for the two markers above it.
    fn layer(self) -> DrawLayer {
        match self {
            Slot::Wheel | Slot::ValueStrip => DrawLayer::Artwork,
            Slot::WheelThumb | Slot::ValueThumb => DrawLayer::Overlay,
        }
    }
}

/// Get-or-create one slot's texture region, `layer` re-layering an existing one; returns its id.
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
            model.touch_layout(); // a region entered the layout gate's read set
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

/// `Set<Slot>Texture([path [, drawLayer]] | r, g, b [, a])`, plus an empty form that only creates
/// the region: what `<ColorWheelTexture/>` means. A file-less slot is where the app renderer paints
/// the disc and strip the reference computes (`0x78b580`, `0x78b8a0`).
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
        // Shape C on r, g, b (`ColorSelect:SetColorRGB 0x78eae0`, `2=C 3=C 4=C 5=B`).
        lua.create_function(|lua, (this, r, g, b): (Table, Value, Value, Value)| {
            let (r, g, b) = (
                crate::script::object::as_f64(&r),
                crate::script::object::as_f64(&g),
                crate::script::object::as_f64(&b),
            );
            // Store through the client's quantizer, then fire with what the widget now holds: the
            // round-tripped values, identical to the next `GetColorRGB`, not the raw arguments.
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

    // The four texture accessors (`0x78de90`, `0x78e160`, `0x78e450`, `0x78e720`; getters
    // `0x78dd80`, `0x78e070`), through which the XML loader installs a ColorSelect's elements.
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

/// Fire `OnColorSelect(self, r, g, b)`; a handler error goes to [`Model::errors`], not the caller.
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

/// The wheel's normalised coordinates for `(x, y)`: the offset from its centre over its
/// half-extents, so a non-square wheel normalises to a disc (`0x78bdd0`); y is up, as the client's.
fn wheel_norm(r: Rect, x: f32, y: f32) -> (f32, f32) {
    let hw = (r.right - r.left) * 0.5;
    let hh = (r.top - r.bottom) * 0.5;
    let cx = (r.left + r.right) * 0.5;
    let cy = (r.bottom + r.top) * 0.5;
    // A zero-size rect gives 0, not NaN, which would poison every colour after it.
    let nx = if hw != 0.0 { (x - cx) / hw } else { 0.0 };
    let ny = if hh != 0.0 { (y - cy) / hh } else { 0.0 };
    (nx, ny)
}

/// The hue and saturation a press or drag at `(x, y)` writes: the wheel's left edge is 0° (red)
/// and its right 180° (cyan), `S` is the radius clamped at the rim, and nothing is quantized.
fn wheel_hs(r: Rect, x: f32, y: f32) -> (f32, f32) {
    let (nx, ny) = wheel_norm(r, x, y);
    let radius = (nx * nx + ny * ny).sqrt();
    let hue = (ny.atan2(nx) + std::f32::consts::PI).to_degrees();
    (hue, radius.min(1.0))
}

/// The brightness a press or drag at `y` writes: the clamped fraction of the way up the strip
/// (`0x78beed`); x is ignored, so a drag may wander sideways off it.
fn strip_value(r: Rect, y: f32) -> f32 {
    let span = r.top - r.bottom;
    if span == 0.0 {
        return 0.0;
    }
    ((y - r.bottom) / span).clamp(0.0, 1.0)
}

/// The wheel marker's rect, seated where the current `(H, S)` came from, the inverse of
/// [`wheel_hs`]: `0x78bc20` sets its `CENTER` at `(−m·cos θ, −m·sin θ)` from the wheel's, with
/// `m = GetWidth(wheel)·0.5·S`, the negations being the pick's `+π`. The width serves both axes,
/// as in the reference, though the pick divides y by half the height. `S == 0` centres it, so the
/// `-1` grey hue sentinel is harmless.
pub(super) fn wheel_thumb_rect(wheel: Rect, thumb_size: Option<(f32, f32)>, hsv: [f32; 3]) -> Rect {
    let (tw, th) = thumb_size.unwrap_or((0.0, 0.0));
    let cx = (wheel.left + wheel.right) * 0.5;
    let cy = (wheel.bottom + wheel.top) * 0.5;
    let m = (wheel.right - wheel.left) * 0.5 * hsv[1];
    let theta = hsv[0].to_radians();
    let (x, y) = (cx - m * theta.cos(), cy - m * theta.sin());
    Rect::new(y - th * 0.5, x - tw * 0.5, y + th * 0.5, x + tw * 0.5)
}

/// The value marker's rect: centred on the strip and `V` of the way up from its bottom
/// (`0x78bcf0`), scaled by the wheel's height, not the strip's, as the reference reads
/// `[this+0x318]`. Deviation: with no wheel it scales by the strip's, because the reference reads
/// the missing wheel unguarded.
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

/// The in-flight colour drag: the reference's two independent flags (`+0x314` wheel, `+0x315`
/// strip), both set when a press hits both; the flag, not the cursor, decides what a move writes.
#[derive(Clone, Copy)]
pub(crate) struct ColorDrag {
    pub(crate) frame: FrameHandle,
    wheel: bool,
    strip: bool,
}

/// The laid-out rects of a ColorSelect's wheel and value strip.
fn hit_rects(model: &Model, h: FrameHandle) -> (Option<Rect>, Option<Rect>) {
    let (wheel, strip) = match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(KindState::ColorSelect(s)) => (s.wheel, s.value_strip),
        _ => (None, None),
    };
    let rect = |rh: Option<RegionHandle>| rh.and_then(|rh| model.region_resolved.get(&rh).copied());
    (rect(wheel), rect(strip))
}

/// On a LeftButton press inside a wheel or strip, capture the drag and apply it at once, as
/// `0x78bf10` does, so a click jumps the colour; returns the id and colour the caller fires.
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

/// On a pointer move during a drag, write the captured H/S and V into the widget's HSV and return
/// the frame id and colour to fire with, on every move: there is no change gate (`0x78bafd`).
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

/// Release a colour drag (LeftButton up or the pointer leaving): `0x78bf90` clears both flags.
pub(super) fn end_drag(model: &mut Model) {
    model.color_drag = None;
}

/// Fire `OnColorSelect` by frame id, for the pointer path, which has no `this` table.
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

    /// White, not black: the constructor sets `H=0, S=0, V=1`.
    #[test]
    fn a_fresh_color_select_is_white() {
        let s = UiScript::new().unwrap();
        s.run(r#"cs = CreateFrame("ColorSelect", "TestColorSelectNew")"#)
            .unwrap();
        let (r, g, b): (f64, f64, f64) = s.eval("return cs:GetColorRGB()").unwrap();
        assert_eq!((r, g, b), (1.0, 1.0, 1.0));
    }

    /// Cyan, a two-channel tie at the maximum, loses one step of green, as in the reference.
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

    /// Reading back and setting walks `(0, 8, 132)` to `(0, 0, 132)`, as in the reference.
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
        // 0 is a fixed point: a ratchet, not a runaway.
        let g: f64 = s
            .eval("local r, g, b = cs:GetColorRGB() cs:SetColorRGB(r, g, b) return g")
            .unwrap();
        assert_eq!(g, 0.0);
    }

    /// The two quantizers, directly. A is round-half-up and clamps; B is a floor with no clamp.
    #[test]
    fn the_two_quantizers_disagree_in_the_verified_way() {
        // A, inbound, clamps: `2` is white and `-1` black, not a wrapped byte.
        assert_eq!(ColorSelectState::quantize_a(-1.0), 0);
        assert_eq!(ColorSelectState::quantize_a(0.0), 0);
        assert_eq!(ColorSelectState::quantize_a(1.0), 255);
        assert_eq!(ColorSelectState::quantize_a(2.0), 255);
        // The half-up bias: 0.5·255 = 127.5, +0.5 = 128.0, truncated = 128.
        assert_eq!(ColorSelectState::quantize_a(0.5), 128);

        // B, outbound: the endpoints agree with A, a half step does not.
        assert_eq!(ColorSelectState::quantize_b(0.0), 0);
        assert_eq!(ColorSelectState::quantize_b(1.0), 255);
        assert_eq!(ColorSelectState::quantize_b(0.5), 127, "floor, not half-up");
        assert_ne!(
            ColorSelectState::quantize_b(0.5),
            ColorSelectState::quantize_a(0.5),
            "the disagreement IS the mechanism behind the drift"
        );
        // B has no clamp: out of range wraps mod 256 (reachable only through `SetColorHSV`).
        assert_eq!(ColorSelectState::quantize_b(-0.001), 255);
    }

    /// It fires with the colour the widget now holds, and again on the same colour: no change gate.
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

        // The handler's arguments equal the next `GetColorRGB()` bit for bit, as in the reference.
        s.run("fired = {} cs:SetColorRGB(0, 1, 1)").unwrap();
        let (from_handler, from_getter): (String, String) = s
            .eval("local r, g, b = cs:GetColorRGB() return fired[1], r .. \",\" .. g .. \",\" .. b")
            .unwrap();
        assert_eq!(from_handler, from_getter);
    }

    #[test]
    fn the_methods_do_not_leak_onto_other_kinds() {
        let s = UiScript::new().unwrap();
        s.run(r#"plain = CreateFrame("Frame", "TestPlainFrame")"#)
            .unwrap();
        assert!(s.eval::<bool>("return plain.SetColorRGB == nil").unwrap());
        assert!(s.eval::<bool>("return plain.GetColorRGB == nil").unwrap());
    }
    // ─────────────────────────────────────────────────────────────────────────────────────────
    // The wheel and the strip: the pick, the drag and the two markers
    // ─────────────────────────────────────────────────────────────────────────────────────────

    /// A ColorSelect with the reference's geometry, laid out: a 128×128 wheel at the top-left and a
    /// 32×128 strip to its right, built through the XML loader so the elements' install path runs.
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

    fn wheel_rect(s: &mut UiScript) -> (f32, f32, f32, f32) {
        let (l, b, w, h): (f32, f32, f32, f32) = s
            .eval(
                "local r = TestPickerWheel; \
                 return r:GetLeft(), r:GetBottom(), r:GetWidth(), r:GetHeight()",
            )
            .unwrap();
        (l, b, w, h)
    }

    /// The wheel's name is a global, which `<ColorValueTexture>`'s anchor resolves against.
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

    /// The disc (`0x78b580`) and the pick (`0x78bdd0`) are inverses: left is red, right cyan, top
    /// violet, bottom chartreuse, and a sign error flips two of the four.
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

    /// White whatever the hue, as the `-1` grey sentinel is inert in `hsv_to_rgb`.
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

    /// The press is the first move, and the capture is a flag, not a hit test per move (`0x78bd80`
    /// is gated on `+0x314`); dragging from red to cyan without lifting proves both.
    #[test]
    fn a_wheel_drag_keeps_the_capture_when_the_cursor_wanders_off() {
        let mut s = picker();
        let (l, b, w, h) = wheel_rect(&mut s);
        let cy = b + h * 0.5;
        s.mouse_button(l + 1.0, cy, "LeftButton", true);
        // Well past the right rim, outside the wheel's rect.
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
        // Black is exact whatever the hue: `hsv_to_rgb` scales every channel by V.
        s.mouse_move(sx, sb + sh + 50.0); // and above it: clamps to full
        let full: (f64, f64, f64) = s.eval("return TestPicker:GetColorRGB()").unwrap();
        assert!(
            full.0 == 1.0 && full.1 < 0.02 && full.2 < 0.02,
            "V clamps at 1 and the hue is untouched, got {full:?}"
        );
        // `w` and the unused wheel bounds are only here to seat the click above.
        let _ = (w, l);
    }

    /// Even a step onto the colour already held fires (`0x78bafd`).
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
            s.mouse_move(cx, cy); // the same point: no change, still fires
        }
        let after_moves: i64 = s.eval("return fires").unwrap();
        assert_eq!(after_moves, 4, "no change-gate: every captured move fires");
    }

    #[test]
    fn a_press_outside_both_rects_captures_nothing() {
        let mut s = picker();
        let before: (f64, f64, f64) = s.eval("return TestPicker:GetColorRGB()").unwrap();
        s.mouse_button(340.0, 20.0, "LeftButton", true); // inside the frame, below both
        s.mouse_move(20.0, 150.0); // then over the wheel, uncaptured
        let after: (f64, f64, f64) = s.eval("return TestPicker:GetColorRGB()").unwrap();
        assert_eq!(after, before, "no capture, no colour change");
    }

    /// Checked through extract: the client places the markers from C++ with no anchors, so the
    /// resolver has nothing to answer. A click lands the wheel marker's centre on the point.
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
        // The value marker rides V from the strip's bottom, scaled by the wheel's height (the
        // reference's `[this+0x318]` read); both are 128 here.
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

    /// The extracted rect of the marker `width` wide: the two share one BLP and differ by size, 10
    /// for the wheel's and 48 for the strip's.
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
