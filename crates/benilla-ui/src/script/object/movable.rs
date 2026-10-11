//! Frame methods: movable and resizable windows, and the drag pump behind `StartMoving` and
//! `StartSizing`. The gesture that usually calls them, `OnDragStart`/`OnDragStop` (`0x76bf70`/
//! `0x76c040`), is [`crate::script::cursor`]'s.
//!
//! The flags are bits of `[frame+0xb4]`, set by `0x76a3c0`: movable `0x100`, resizable `0x200`,
//! userPlaced `0x1000`. Every drag enters at `0x7652b0`, which pins the frame to one TOPLEFT
//! anchor against the screen root at its current rect, with an explicit size (`0x768430`), then
//! raises it, sets userPlaced and takes the one drag slot (`root+0xcfc`) with its grip (`+0xd04`).
//! The pump (`0x7655b0` → `0x768710`) works on that anchor and size: a move, grip CENTER, adds
//! each cursor delta over the frame's scale to the anchor's offsets (case 4), and each of the
//! eight grips sizes the frame and re-pins it shifted by its left and top travel only. Stopping
//! (`0x765640`) writes nothing, so after any drag `GetPoint` answers that one TOPLEFT.

use mlua::{Lua, MultiValue, Table, Value};

use crate::layout::{Anchor, Anchors, Point};
use crate::script::{Model, UiScript, SCREEN};
use crate::widget::FrameHandle;

use super::layout_methods::eff_scale;
use super::{frame_handle_of, point_from_str};

/// The one move or size in flight, the reference's drag slot, held in [`Model::moving`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FrameMove {
    /// The frame being dragged (`root+0xcfc`).
    frame: FrameHandle,
    /// The grip (`root+0xd04`): CENTER moves the frame, any other point resizes it from there.
    grip: Point,
    /// The cursor position at the last pump (`root+0xd08`/`+0xd0c`), UI px, y-up.
    sample: (f32, f32),
    /// Whether mouse-up ends the drag, the drag mode as Lua sees it: `0x766420` ends modes 1
    /// (modifier drag) and 2 (title region), never 3 (`StartMoving`, `StartSizing`).
    pub(crate) auto_stop: bool,
}

/// Populate `m`'s movable and resizable methods.
pub(super) fn install(lua: &Lua, m: &Table) -> mlua::Result<()> {
    // SetMovable / IsMovable (`0x776420`/`0x7764d0`): a pure flag write, so clearing it mid-drag
    // stops nothing. The argument is Lua truthiness, as the reference's `toboolean`.
    m.set(
        "SetMovable",
        lua.create_function(|lua, (this, flag): (Table, bool)| {
            set_flag(lua, &this, Flag::Movable, flag)
        })?,
    )?;
    m.set(
        "IsMovable",
        lua.create_function(|lua, this: Table| get_flag(lua, &this, Flag::Movable))?,
    )?;
    // SetResizable / IsResizable (`0x776590`/`0x776640`): the same pure flag write.
    m.set(
        "SetResizable",
        lua.create_function(|lua, (this, flag): (Table, bool)| {
            set_flag(lua, &this, Flag::Resizable, flag)
        })?,
    )?;
    m.set(
        "IsResizable",
        lua.create_function(|lua, this: Table| get_flag(lua, &this, Flag::Resizable))?,
    )?;
    // ── Resize bounds ──
    //
    // `SetMinResize 0x776020`, `GetMinResize 0x775f20`, `SetMaxResize 0x7762a0`,
    // `GetMaxResize 0x7761a0`. Both arguments must pass `lua_isnumber` (`0x6f34d0`), else `Usage:`
    // (`0x8797b8`/`0x8797e4`); the pair is stored raw, with no layout touch and no `min > max`
    // check. The getters always answer two numbers, 0 meaning unbounded.
    for (name, upper) in [("MinResize", false), ("MaxResize", true)] {
        m.set(
            format!("Set{name}"),
            lua.create_function(move |lua, args: MultiValue| {
                let mut it = args.into_iter();
                let this = match it.next() {
                    Some(Value::Table(t)) => t,
                    _ => return Err(mlua::Error::runtime("expected a frame")),
                };
                let h = frame_handle_of(lua, &this)?;
                let pair = (it.next(), it.next());
                // `lua_isnumber`: a number, or a string Lua 5.0's `luaV_tonumber` converts.
                let num = |v: &Option<Value>| match v {
                    Some(Value::Number(n)) => Some(*n as f32),
                    Some(Value::Integer(i)) => Some(*i as f32),
                    Some(Value::String(s)) => s.to_str().ok()?.trim().parse::<f32>().ok(),
                    _ => None,
                };
                let (Some(w), Some(hgt)) = (num(&pair.0), num(&pair.1)) else {
                    let model = lua.app_data_ref::<Model>().expect("model");
                    let who = model
                        .arena
                        .frame(h)
                        .and_then(|f| f.name.clone())
                        .unwrap_or_else(|| "<unnamed>".to_string());
                    let (verb, a, b) = if upper {
                        ("SetMaxResize", "maxWidth", "maxHeight")
                    } else {
                        ("SetMinResize", "minWidth", "minHeight")
                    };
                    return Err(mlua::Error::runtime(format!(
                        "Usage: {who}:{verb}({a}, {b})"
                    )));
                };
                let mut model = lua.app_data_mut::<Model>().expect("model");
                if let Some(f) = model.arena.frame_mut(h) {
                    if upper {
                        f.max_resize = (w, hgt);
                    } else {
                        f.min_resize = (w, hgt);
                    }
                }
                Ok(())
            })?,
        )?;
        m.set(
            format!("Get{name}"),
            lua.create_function(move |lua, this: Table| {
                let h = frame_handle_of(lua, &this)?;
                let model = lua.app_data_ref::<Model>().expect("model");
                let (w, hgt) = model.arena.frame(h).map_or((0.0, 0.0), |f| {
                    if upper {
                        f.max_resize
                    } else {
                        f.min_resize
                    }
                });
                Ok((f64::from(w), f64::from(hgt)))
            })?,
        )?;
    }

    // SetUserPlaced / IsUserPlaced (`0x776a50`/`0x776b40`): the bit that makes the layout cache
    // save the frame's position; the one guarded setter, refusing a frame that is neither movable
    // nor resizable (`0x776adb`).
    m.set(
        "SetUserPlaced",
        lua.create_function(|lua, (this, flag): (Table, bool)| {
            let h = frame_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let ok = model
                .arena
                .frame(h)
                .is_some_and(|f| f.movable || f.resizable);
            if !ok {
                return Err(not_flagged(&model, h, "movable or resizable"));
            }
            if let Some(f) = model.arena.frame_mut(h) {
                if f.user_placed != flag {
                    f.user_placed = flag;
                    // Either transition owes the layout cache a rewrite: a row added or removed.
                    model.user_placed_changed = true;
                }
            }
            Ok(())
        })?,
    )?;
    m.set(
        "IsUserPlaced",
        lua.create_function(|lua, this: Table| get_flag(lua, &this, Flag::UserPlaced))?,
    )?;
    m.set(
        "StartMoving",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            start_moving(&mut model, h)
        })?,
    )?;
    // StartSizing([point]) (`0x776830`; stock caller `FloatingChatFrame.lua:600`): a resize from
    // the named grip, ended by `StopMovingOrSizing`. The grip starts as BOTTOMRIGHT (`0x776913`)
    // and only a string naming a point, in any case, replaces it (`0x77691a`, `0x6f1840`).
    m.set(
        "StartSizing",
        lua.create_function(|lua, (this, point): (Table, Value)| {
            let h = frame_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            if !model.arena.frame(h).is_some_and(|f| f.resizable) {
                return Err(not_flagged(&model, h, "resizable"));
            }
            // The same `root+0xcfc` guard `StartMoving` carries (`0x776947`).
            if model.moving.is_some() {
                return Ok(());
            }
            let grip = match &point {
                Value::String(s) => s.to_str().ok().and_then(|t| point_from_str(&t)),
                _ => None,
            }
            .unwrap_or(Point::BottomRight);
            // Drag mode 3, which mouse-up never ends.
            begin_drag(&mut model, h, grip, false);
            Ok(())
        })?,
    )?;
    // StopMovingOrSizing (`0x776990` → `0x765640`): clears the slot only when it holds this frame.
    m.set(
        "StopMovingOrSizing",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            if model.moving.is_some_and(|mv| mv.frame == h) {
                model.moving = None;
            }
            Ok(())
        })?,
    )?;
    Ok(())
}

/// A bit of the frame's flag word (`[frame+0xb4]`), kept here as an arena field.
#[derive(Clone, Copy)]
enum Flag {
    /// `0x100`.
    Movable,
    /// `0x200`.
    Resizable,
    /// `0x1000`.
    UserPlaced,
}

fn set_flag(lua: &Lua, this: &Table, which: Flag, value: bool) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model");
    if let Some(f) = model.arena.frame_mut(h) {
        match which {
            Flag::Movable => f.movable = value,
            Flag::Resizable => f.resizable = value,
            Flag::UserPlaced => f.user_placed = value,
        }
    }
    Ok(())
}

/// `IsMovable`/`IsResizable`/`IsUserPlaced`: 1 or nil, never a boolean.
fn get_flag(lua: &Lua, this: &Table, which: Flag) -> mlua::Result<Value> {
    let h = frame_handle_of(lua, this)?;
    let model = lua.app_data_ref::<Model>().expect("model");
    Ok(crate::script::binding_abi::flag(
        model.arena.frame(h).is_some_and(|f| match which {
            Flag::Movable => f.movable,
            Flag::Resizable => f.resizable,
            Flag::UserPlaced => f.user_placed,
        }),
    ))
}

/// The family's refusal, naming the frame as the reference does. The wording is ours: the strings
/// at `0x879810`, `0x879828` and `0x879844` are unread, known only by their symbol names.
fn not_flagged(model: &Model, h: FrameHandle, want: &str) -> mlua::Error {
    let who = model
        .arena
        .frame(h)
        .and_then(|f| f.name.clone())
        .unwrap_or_else(|| "<anonymous>".into());
    mlua::Error::runtime(format!("Frame {who} is not {want}"))
}

/// `StartMoving` (`0x776700` → `0x7652b0`): errors on a frame that is not movable (`0x77678b`),
/// ignores a second drag, then starts a move, grip CENTER (`0x776819`).
fn start_moving(model: &mut Model, h: FrameHandle) -> mlua::Result<()> {
    if !model.arena.frame(h).is_some_and(|f| f.movable) {
        return Err(not_flagged(model, h, "movable"));
    }
    // The `root+0xcfc != 0` guard (`0x7767e8`); what the reference does beyond not starting a
    // second drag is untraced.
    if model.moving.is_some() {
        return Ok(());
    }
    // Mode 3: only `StopMovingOrSizing` ends it.
    begin_drag(model, h, Point::Center, false);
    Ok(())
}

/// Begin a title-region move, drag mode 2 (`0x7662c0` → `0x765320` → `0x7652b0`), with no movable
/// check, since nothing on that path reads the bit; `0x765320` asks for grip CENTER when it is not
/// sizing (`0x765332` → `0x765464`). Neither reads the slot first, so a drag in flight is displaced
/// and nothing runs on its frame. False when the frame has no rect, so nothing started.
pub(crate) fn start_title_move(model: &mut Model, h: FrameHandle) -> bool {
    begin_drag(model, h, Point::Center, true)
}

/// Begin the layout-mode modifier drag on the toplevel window `h` that `0x765490`'s walk lifted,
/// from its flag test on: Control held sizes and needs the window resizable (`0x765570`), else it
/// moves and needs it movable (`0x765560`). Then `0x765320` with drag mode 1, which the release
/// ends: grip CENTER for a move, the cell of `grid_grip` for a size. False when nothing started.
pub(crate) fn start_layout_drag(
    model: &mut Model,
    h: FrameHandle,
    at: (f32, f32),
    sizing: bool,
) -> bool {
    let allowed = model
        .arena
        .frame(h)
        .is_some_and(|f| if sizing { f.resizable } else { f.movable });
    if !allowed {
        return false;
    }
    let grip = if sizing {
        grid_grip(model, h, at)
    } else {
        Some(Point::Center)
    };
    grip.is_some_and(|grip| begin_drag(model, h, grip, true))
}

/// `0x765320`'s sizing grip: with the layout resolved (`0x76533e` → `0x7680e0`), the cell of a 3×3
/// grid of the frame's rect (`0x768320`) that `at` falls in, each axis cut at a quarter and three
/// quarters (`0.25` at `0x8029b0`, `3.0` at `0x80297c`), a cut belonging to the cell past it; the
/// middle cell is CENTER, a move (`0x765384`–`0x765464`). The x offset and the quarter-width stay
/// in x87 registers, the y offset and the quarter-height are stored as floats (`0x765390`,
/// `0x7653ab`). None when the frame has no rect (`0x765379`).
fn grid_grip(model: &mut Model, h: FrameHandle, (x, y): (f32, f32)) -> Option<Point> {
    UiScript::resolve_layout(model);
    let r = *model.resolved.get(&h)?;
    let dx = f64::from(x) - f64::from(r.left);
    let quarter_w = (f64::from(r.right) - f64::from(r.left)) * 0.25;
    let dy = f64::from((f64::from(y) - f64::from(r.bottom)) as f32);
    let quarter_h = f64::from(((f64::from(r.top) - f64::from(r.bottom)) * 0.25) as f32);
    // 0 the near cell (left, bottom), 1 the middle, 2 the far.
    let cell = |d: f64, quarter: f64| {
        if d < quarter {
            0
        } else if d >= quarter * 3.0 {
            2
        } else {
            1
        }
    };
    Some(match (cell(dx, quarter_w), cell(dy, quarter_h)) {
        (0, 2) => Point::TopLeft,
        (1, 2) => Point::Top,
        (2, 2) => Point::TopRight,
        (0, 1) => Point::Left,
        (2, 1) => Point::Right,
        (0, 0) => Point::BottomLeft,
        (1, 0) => Point::Bottom,
        (2, 0) => Point::BottomRight,
        _ => Point::Center,
    })
}

/// The drag entry `0x7652b0`: pin the frame (`0x7652c6` → `0x768430`) and, only when it has a rect
/// to pin (`0x7652cd`), raise it (`0x7652d7`), set userPlaced (`0x7652e5`) and take the slot
/// (`0x7652f3`–`0x76530f`) over whatever held it. False when the frame has no rect, so nothing
/// started.
fn begin_drag(model: &mut Model, h: FrameHandle, grip: Point, auto_stop: bool) -> bool {
    let Some(repinned) = pin_top_left(model, h, (0.0, 0.0), (0.0, 0.0)) else {
        return false;
    };
    // The worker applies the toplevel gate.
    super::toplevel::raise(model, h);
    // The layout cache is dirtied on the bit's transition only, since every title-region click
    // runs this, and again when the pin rewrote what a saved row holds.
    if let Some(f) = model.arena.frame_mut(h) {
        if !f.user_placed {
            f.user_placed = true;
            model.user_placed_changed = true;
        }
    }
    if repinned {
        mark_user_placed_change(model, h);
    }
    model.moving = Some(FrameMove {
        frame: h,
        grip,
        sample: model.cursor_pos,
        auto_stop,
    });
    true
}

/// `0x768430` against the screen root. With the layout settled first (`0x768464`–`0x76846f`), read
/// the frame's rect (`0x76847a`), clear its anchors (`0x767620`), set one TOPLEFT on the screen's
/// TOPLEFT at the rect's top-left moved by `travel` (`0x767c70`), and give it an explicit size,
/// `size` per axis or the rect's where that is 0 (`0x7684e1`, `0x768508`). Offsets and size are in
/// the frame's own units, screen over its effective scale (`[G+0x58]`). None when the frame has no
/// rect (`0x768481`); else whether its anchors or size changed.
fn pin_top_left(
    model: &mut Model,
    h: FrameHandle,
    size: (f32, f32),
    travel: (f32, f32),
) -> Option<bool> {
    UiScript::resolve_layout(model);
    let r = *model.resolved.get(&h)?;
    let root = model.screen;
    let scale = f64::from(eff_scale(model, h));
    let inv = 1.0 / scale;
    let x = ((f64::from(r.left) + f64::from(travel.0)) - f64::from(root.left)) * inv;
    let y = ((f64::from(r.top) + f64::from(travel.1)) - f64::from(root.top)) * inv;
    let w = if size.0 == 0.0 {
        ((f64::from(r.right) - f64::from(r.left)) / scale) as f32
    } else {
        size.0
    };
    let ht = if size.1 == 0.0 {
        ((f64::from(r.top) - f64::from(r.bottom)) / scale) as f32
    } else {
        size.1
    };
    let pin = Anchors::from([Anchor::new(
        Point::TopLeft,
        SCREEN,
        Point::TopLeft,
        x as f32,
        y as f32,
    )]);
    let input = model.layout_inputs.entry(h).or_default();
    let changed = !input.anchors.bits_eq(&pin)
        || input.width.to_bits() != w.to_bits()
        || input.height.to_bits() != ht.to_bits();
    input.anchors = pin;
    input.width = w;
    input.height = ht;
    // The size setters and a closing `0x7680e0(1)` invalidate (`0x768533`).
    model.invalidate_frame(h, changed);
    // The size it now carries is explicit, as a `SetWidth`/`SetHeight` would leave it.
    model.note_authored_size(h);
    Some(changed)
}

/// A drag moved the frame: if it is user-placed, the layout cache owes its file a rewrite.
fn mark_user_placed_change(model: &mut Model, h: FrameHandle) {
    if model.arena.frame(h).is_some_and(|f| f.user_placed) {
        model.user_placed_changed = true;
    }
}

/// The hover walk's move/size branch (`0x766164`–`0x766193`): while the drag slot holds a target,
/// the walk pumps it to `pos` (`0x7655b0`) and returns before any probe. True when one is in
/// flight, a target whose frame is gone dropped first.
pub(crate) fn pump_move_or_size(model: &mut Model, pos: (f32, f32)) -> bool {
    let Some(mv) = model.moving else {
        return false;
    };
    if model.arena.frame(mv.frame).is_none() {
        model.moving = None;
        return false;
    }
    let (dx, dy) = (pos.0 - mv.sample.0, pos.1 - mv.sample.1);
    // A zero delta on both axes skips the step (`0x7655d6`–`0x7655f4`).
    if dx != 0.0 || dy != 0.0 {
        drag_step(model, mv, dx, dy);
    }
    // The sample takes the whole delta, whatever a bound kept of it (`0x765610`–`0x765628`).
    model.moving = Some(FrameMove {
        sample: (mv.sample.0 + dx, mv.sample.1 + dy),
        ..mv
    });
    true
}

/// A hide ends the move or size whose target is the frame going hidden: the hide's de-registration
/// (`0x76ad9d` → `0x764920`, flag 0) clears the slot when it holds the frame
/// (`0x76492e`–`0x76493a` → `0x765640`).
pub(crate) fn cancel_on_hide(model: &mut Model, h: FrameHandle) {
    if model.moving.is_some_and(|mv| mv.frame == h) {
        model.moving = None;
    }
}

/// One pump step, `0x768710` by way of `0x76a660`, for the cursor delta `(dx, dy)` in screen units.
/// The grips read the explicit size (`vtbl+0x1c`/`+0x20`, `G+0x50`/`+0x54`), size it by the travel
/// over the frame's scale, clamp it, and re-pin TOPLEFT shifted by the left and top travel only
/// (`0x768974` → `0x768430`); a move adds to the TOPLEFT anchor itself.
fn drag_step(model: &mut Model, mv: FrameMove, dx: f32, dy: f32) {
    let h = mv.frame;
    let scale = f64::from(eff_scale(model, h));
    let inv = 1.0 / scale;
    let (mut w, mut ht) = model
        .layout_inputs
        .get(&h)
        .map_or((0.0, 0.0), |i| (i.width, i.height));
    // `G+0x60`/`+0x68` bound the width and `G+0x5c`/`+0x64` the height.
    let (min, max) = model
        .arena
        .frame(h)
        .map_or(((0.0, 0.0), (0.0, 0.0)), |f| (f.min_resize, f.max_resize));
    let (mut dx, mut dy) = (dx, dy);
    let add = |v: f32, by: f64| (f64::from(v) + by) as f32;
    let x = Axis::new(min.0, max.0, scale);
    let y = Axis::new(min.1, max.1, scale);
    match mv.grip {
        // Case 4: the delta over the scale goes straight into the TOPLEFT anchor's offsets
        // (`0x768c15`–`0x768c2e`); with no TOPLEFT anchor it re-pins at the whole delta instead
        // (`0x768c00`, `0x768c0f`).
        Point::Center => {
            let moved = model
                .layout_inputs
                .get_mut(&h)
                .and_then(|i| i.anchors.offset_mut(Point::TopLeft))
                .map(|(x_off, y_off)| {
                    *x_off = add(*x_off, f64::from(dx) / scale);
                    *y_off = add(*y_off, f64::from(dy) / scale);
                })
                .is_some();
            if moved {
                // The offsets move and the frame is invalidated (`0x768c31`).
                model.invalidate_frame(h, true);
                mark_user_placed_change(model, h);
                return;
            }
        }
        Point::TopLeft => {
            w = add(w, -f64::from(dx) * inv);
            x.clamp_near(&mut w, &mut dx, -1.0);
            ht = add(ht, f64::from(dy) * inv);
            y.clamp_near(&mut ht, &mut dy, 1.0);
        }
        Point::Top => {
            ht = add(ht, f64::from(dy) / scale);
            y.clamp_near(&mut ht, &mut dy, 1.0);
            dx = 0.0;
        }
        Point::TopRight => {
            w = add(w, f64::from(dx) * inv);
            x.clamp_far(&mut w);
            dx = 0.0;
            ht = add(ht, f64::from(dy) * inv);
            y.clamp_near(&mut ht, &mut dy, 1.0);
        }
        Point::Left => {
            w = add(w, -f64::from(dx) / scale);
            x.clamp_near(&mut w, &mut dx, -1.0);
            dy = 0.0;
        }
        Point::Right => {
            w = add(w, f64::from(dx) / scale);
            x.clamp_far(&mut w);
            (dx, dy) = (0.0, 0.0);
        }
        Point::BottomLeft => {
            w = add(w, -f64::from(dx) * inv);
            x.clamp_near(&mut w, &mut dx, -1.0);
            ht = add(ht, -f64::from(dy) * inv);
            y.clamp_far(&mut ht);
            dy = 0.0;
        }
        Point::Bottom => {
            ht = add(ht, -f64::from(dy) / scale);
            y.clamp_far(&mut ht);
            (dx, dy) = (0.0, 0.0);
        }
        Point::BottomRight => {
            w = add(w, f64::from(dx) * inv);
            x.clamp_far(&mut w);
            ht = add(ht, -f64::from(dy) * inv);
            y.clamp_far(&mut ht);
            (dx, dy) = (0.0, 0.0);
        }
    }
    if pin_top_left(model, h, (w, ht), (dx, dy)) == Some(true) {
        mark_user_placed_change(model, h);
    }
}

/// One axis's `SetMinResize`/`SetMaxResize` pair, as `0x768710` applies it: only an exact 0.0 is
/// unbounded, a size under the minimum or over the maximum takes the bound, min before max so max
/// wins a crossed pair, and with no bound there is no floor, so a size can go negative.
struct Axis {
    min: f32,
    max: f32,
    scale: f64,
}

impl Axis {
    fn new(min: f32, max: f32, scale: f64) -> Axis {
        Axis { min, max, scale }
    }

    /// Clamp a size whose far edge moves (the right or the bottom), which the pin does not follow.
    fn clamp_far(&self, v: &mut f32) {
        if self.min != 0.0 && *v < self.min {
            *v = self.min;
        }
        if self.max != 0.0 && *v > self.max {
            *v = self.max;
        }
    }

    /// Clamp a size whose anchored edge moves (the left or the top), taking the residual back out
    /// of that edge's travel so it stops on the bound, as TOPLEFT's case does at `0x768770`,
    /// `0x7687a2`, `0x7687dd` and `0x768817`. `grows` is the sign the travel sizes the frame with:
    /// +1 for the top edge, -1 for the left.
    fn clamp_near(&self, v: &mut f32, travel: &mut f32, grows: f64) {
        let mut seat = |v: &mut f32, b: f32| {
            *travel =
                (f64::from(*travel) + grows * (f64::from(b) - f64::from(*v)) * self.scale) as f32;
            *v = b;
        };
        if self.min != 0.0 && *v < self.min {
            seat(v, self.min);
        }
        if self.max != 0.0 && *v > self.max {
            seat(v, self.max);
        }
    }
}
