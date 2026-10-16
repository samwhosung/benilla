//! The wheel walk (`0x7664f0`, the handler of category `0x10`). One sweep over the frames that
//! took the wheel (the input mask's bit 3, a list apart from the mouse's) whose rect holds the
//! cursor, strata from TOOLTIP down to WORLD (`0x766550`–`0x7665a3`), and in a stratum by level
//! high to low, equal levels in the order they entered the list (`0x764aa0`). Each is offered the
//! notch through its wheel slot (`0x766586`, `0x766597`) until one answers nonzero.
//!
//! A plain frame (`0x76c180`) answers only when it is visible and has an `OnMouseWheel` script
//! (`0x76c183`–`0x76c19b`); otherwise it answers 0 and the sweep goes on (`0x76c1cb`), whatever its
//! mouse bit, its parents or its siblings hold. The world frame (`0x483d20`) runs the base slot
//! first and, with no script, the `MOUSEWHEELUP`/`MOUSEWHEELDOWN` binding (`0x483d53`–`0x483d82`):
//! the camera zoom. So a notch over a frame with no `OnMouseWheel` ends at the world.

use mlua::Value;

use crate::widget::{FrameHandle, FrameKind};

use super::pointer::frame_contains;
use super::{event, Model, UiScript};

/// Where a wheel notch ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WheelWalk {
    /// A frame's `OnMouseWheel` took the notch (`0x76c1c1`).
    Frame,
    /// Nothing took it before the world frame, whose slot runs the wheel binding (`0x483d53`);
    /// the host runs it and the walk stops here.
    World,
    /// No frame took it: the world frame is not under the cursor (none exists, as on the glue
    /// screens) or its chord has no binding (`0x483d87`) and nothing behind it took the notch.
    Declined,
}

/// The frames of the wheel list in the order `0x7664f0` offers them the notch: the wheel-enabled,
/// effectively visible frames (`0x76af00` enters a frame in the list only while it is visible),
/// strata high to low, then level high to low, then oldest entry first.
fn walk_order(model: &Model) -> Vec<FrameHandle> {
    let mut list: Vec<(FrameHandle, u8, u16, u64)> = model
        .arena
        .iter_frames()
        .filter(|(_, f)| f.effective_visible && f.mouse_wheel_enabled)
        .map(|(h, f)| (h, f.strata as u8, f.level, f.wheel_stamp))
        .collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then(a.3.cmp(&b.3)));
    list.into_iter().map(|(h, ..)| h).collect()
}

impl UiScript {
    /// [`Self::mouse_wheel_bound`] for a world frame whose chord has a binding.
    pub fn mouse_wheel(&mut self, x: f32, y: f32, delta: f32) -> WheelWalk {
        self.mouse_wheel_bound(x, y, delta, true)
    }

    /// One wheel notch at the cursor `(x, y)` (UI units, y up): the sweep `0x7664f0`. A frame with
    /// an `OnMouseWheel` script runs it with `arg1` the sign of `delta`, `1` for zero or more and
    /// `-1` below (`0x76c1a9`), and ends the sweep. The world frame ends it with
    /// [`WheelWalk::World`] unless it has such a script too; a zero `delta` ends it there with no
    /// binding (`0x483d47`). `world_binds` says whether the notch's chord has a binding: the
    /// world slot answers the sum of its two `ExecuteBinding` results (`0x483d87`), so with none
    /// it answers 0 and the walk goes on to the wheel frames behind the world frame.
    pub fn mouse_wheel_bound(
        &mut self,
        x: f32,
        y: f32,
        delta: f32,
        world_binds: bool,
    ) -> WheelWalk {
        let order = walk_order(&self.model_ref());
        for fh in order {
            let (id, world) = {
                let model = self.model_ref();
                if !frame_contains(&model, fh, x, y) {
                    continue;
                }
                (
                    model.frame_to_id.get(&fh).copied(),
                    model
                        .arena
                        .frame(fh)
                        .is_some_and(|f| f.kind == FrameKind::WorldFrame),
                )
            };
            if let Some(id) =
                id.filter(|&id| event::has_widget_handler(&self.lua, id, "OnMouseWheel"))
            {
                let sign = if delta >= 0.0 { 1.0 } else { -1.0 };
                if let Err(e) = event::fire_widget_handler(
                    &self.lua,
                    id,
                    "OnMouseWheel",
                    vec![Value::Number(sign)],
                ) {
                    self.push_error(e);
                }
                return WheelWalk::Frame;
            }
            if world {
                if delta == 0.0 {
                    return WheelWalk::Frame;
                }
                if world_binds {
                    return WheelWalk::World;
                }
            }
        }
        WheelWalk::Declined
    }
}
