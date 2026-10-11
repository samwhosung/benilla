//! The world mouseover's publisher (`0x492890`) and the engine half of a hover leave. The world's
//! pick publishes through the change check (`0x482090`), and a leave publishes inside the hover
//! walk, which runs the old frame's leave before the new frame's enter (`0x766229`, `0x766239`):
//! the WorldFrame's (`0x483b10`) and a nameplate's (`0x7cb8b0`) publish the empty mouseover, and
//! the Minimap's (`0x4eb880`) hides `GameTooltip`. So the tooltip a window's `OnEnter` takes is
//! never torn down by the hover it replaced.
//!
//! The WorldFrame's enter is the base one (`0x76b6a0`) and publishes nothing: the world gains the
//! mouseover from its own update (`0x481790`), which picks only while the WorldFrame holds the
//! hover (`0x4817ae`), the app's world drive.

use mlua::Lua;

use crate::widget::{FrameHandle, FrameKind};

use super::{event, Model, UiScript};

/// What a frame's leave notify (`vtbl+0x50`) does beyond its `OnLeave`.
enum LeaveClass {
    /// `0x483b10`: the base leave, then the empty mouseover.
    World,
    /// `0x7cb8b0`: the empty mouseover, then the Button's leave.
    Nameplate,
    /// `0x4eb880`: `GameTooltip` hidden, then the base leave.
    Minimap,
    Other,
}

fn leave_class(model: &Model, h: FrameHandle) -> LeaveClass {
    match model.arena.frame(h).map(|f| f.kind) {
        Some(FrameKind::WorldFrame) => LeaveClass::World,
        Some(FrameKind::Minimap) => LeaveClass::Minimap,
        Some(_) if model.nameplates.is_plate(h) => LeaveClass::Nameplate,
        _ => LeaveClass::Other,
    }
}

/// A frame's leave notify, by its class, around its `OnLeave`; `script` is the id whose `OnLeave`
/// fires, `None` where the notify skips it (a disabled Button's `0x7794e0`). The walk's motion
/// leave and the removal tail's leave (`0x764cce`) both come here. The notify's own argument, 0 for
/// a motion leave and 1 for a removal, only clears the drag arm (`0x76b725`); `OnLeave` runs
/// through the plain runner (`0x76b753` → `0x702690`), which sets no arg global.
pub(super) fn leave(lua: &Lua, h: FrameHandle, script: Option<u32>) {
    let class = leave_class(&model(lua), h);
    match class {
        // `0x7cb8bf`: `0x492890` with an empty mouseover and an empty "previous", first.
        LeaveClass::Nameplate => {
            publish(lua, None);
        }
        // `0x4eb88e`–`0x4eb898`: the shown bit, then the hide slot, whoever holds it.
        LeaveClass::Minimap => hide_tooltip(lua),
        LeaveClass::World | LeaveClass::Other => {}
    }
    if let Some(id) = script {
        if let Err(e) = event::fire_widget_handler(lua, id, "OnLeave", Vec::new()) {
            model(lua).record_lua_error(&e);
        }
    }
    // `0x483b1d` runs the base leave first; then, only while the WorldFrame's own last publish is
    // not empty (`0x483b30`), the empty one (`0x483b46`).
    if let LeaveClass::World = class {
        publish(lua, None);
    }
}

/// The publisher behind the change check: an unchanged mouseover does nothing (`0x4820b5`). A
/// change away from a published one requests the fade on `GameTooltip` (`0x4928d5`, `0x492909` →
/// `0x530ae0`), whoever holds it, after `"mouseover"` stops naming the old unit (the pair is zeroed
/// at `0x4928e8` first). Returns whether it changed.
pub(super) fn publish(lua: &Lua, guid: Option<u64>) -> bool {
    let (prev, tooltip) = {
        let mut model = model(lua);
        if model.world_mouseover == guid {
            return false;
        }
        let prev = std::mem::replace(&mut model.world_mouseover, guid);
        // The token resolver reads the pair (`0x515970`), so `"mouseover"` names a unit only while
        // it is the published guid; the app pushes the new unit after the publish.
        if model
            .pushed_unit("mouseover")
            .is_some_and(|u| Some(u.guid) != guid)
        {
            model.units_by_lower.remove("mouseover");
        }
        if model.unit_guids.mouseover != 0 && Some(model.unit_guids.mouseover) != guid {
            model.unit_guids.mouseover = 0;
        }
        (prev, model.arena.lookup("GameTooltip"))
    };
    if let (Some(_), Some(h)) = (prev, tooltip) {
        super::tooltip::fade_out(lua, h);
    }
    true
}

/// The Minimap's direct hide of `GameTooltip`: the shown bit, then the hide slot, with no fade
/// (`0x4eb15e`–`0x4eb166`, `0x4eb55f`–`0x4eb567`, `0x4eb88e`–`0x4eb898`).
fn hide_tooltip(lua: &Lua) {
    let tooltip = model(lua).arena.lookup("GameTooltip");
    if let Some(h) = tooltip {
        super::visibility::set_shown(lua, h, false);
    }
}

/// The Minimap's probe (`0x4eb0c0`, its `vtbl+0x3c`) found the cursor outside it: a blip still
/// held is dropped and `GameTooltip` hidden (`0x4eb0ee`, `0x4eb12b`–`0x4eb166`). The walk probes
/// the Minimap while it holds the capture (`0x7661ca`), and in the sweep when no frame ahead of it
/// took the hit.
pub(super) fn minimap_probe_miss(lua: &Lua) {
    if model(lua).minimap_blip.take().is_some() {
        hide_tooltip(lua);
    }
}

fn model(lua: &Lua) -> mlua::AppDataRefMut<'_, Model> {
    lua.app_data_mut::<Model>().expect("model app_data")
}

impl UiScript {
    /// The world mouseover's guid as last published (`[0xb4e2c8]`), a unit, a GameObject or a
    /// corpse; `None` once the pick or a hover leave published the empty one.
    pub fn world_mouseover(&self) -> Option<u64> {
        self.model_ref().world_mouseover
    }

    /// Publish the world pick's mouseover ([`publish`]): the caller then pushes a unit's
    /// `"mouseover"` and builds its plate. Returns whether it changed.
    pub fn publish_world_mouseover(&mut self, guid: Option<u64>) -> bool {
        publish(&self.lua, guid)
    }

    /// Whether the walk probes the Minimap with the cursor on it: it holds the hover, which a hit
    /// in the sweep makes it, or the capture, whose probe runs before the sweep (`0x7661ca`). Its
    /// probe shows and drops the blip tooltip.
    pub fn minimap_probed(&self) -> bool {
        let model = self.model_ref();
        [model.mouseover, model.mouse_capture]
            .into_iter()
            .flatten()
            .filter_map(|h| model.arena.frame(h))
            .any(|f| f.kind == FrameKind::Minimap)
    }

    /// The blip whose tooltip [`Self::minimap_tooltip`] put up, while the probe holds it.
    pub fn minimap_blip(&self) -> Option<String> {
        self.model_ref().minimap_blip.clone()
    }

    /// The probe found no blip under the cursor inside the Minimap: it drops the held one and hides
    /// `GameTooltip`, whoever holds it (`0x4eb54c`–`0x4eb567`).
    pub fn minimap_blip_lost(&mut self) {
        if self.model_mut().minimap_blip.take().is_some() {
            hide_tooltip(&self.lua);
        }
    }
}
