//! The UI scale: `UIParent`'s own frame scale, never the screen's. The screen root keeps scale 1
//! and is 768 units tall at every window size; `uiScale` reaches only `UIParent`'s subtree,
//! through the worker Lua's `SetScale` calls (`0x494550` → `0x76ac10` on the frame named
//! `"UIParent"`, looked up at `0x49006a`), so a frame with no parent, or under `WorldFrame`, draws
//! at scale 1.

use super::{Model, UiScript};

/// `SetUiScale`'s floor (`0x494550`: `[0x804584]`, below which `0x494563` stores 0.64).
pub(crate) const UI_SCALE_FLOOR: f32 = 0.64;

/// `GetUiScale` (`0x494590`): `UIParent`'s effective scale (`[UIParent+0x7c]`), else 1.0 when there
/// is no `UIParent`. `GetScreenWidth`/`GetScreenHeight` (`0x48b480`/`0x48b4d0`) divide the root by
/// it, and nothing else reads it. A zero scale reads 1, as the reference's writer `0x768360`
/// refuses one.
pub(crate) fn ui_parent_scale(model: &Model) -> f32 {
    model
        .arena
        .lookup("UIParent")
        .map_or(1.0, |h| super::object::eff_scale(model, h))
}

/// The scale `uiScale`'s callback hands the sink (`0x490770`, and the display handler's ON leg
/// `0x492e90`): below a 4:3 aspect `a` it is capped at `0.75·a` (`a·3 ≥ 4` skips the cap, so 4:3
/// itself is never capped), then floored at [`UI_SCALE_FLOOR`] (`0x494550`).
pub(crate) fn effective_ui_scale(ui_scale: f32, aspect: f32) -> f32 {
    let mut v = ui_scale;
    if aspect * 3.0 < 4.0 {
        v = v.min(aspect * 0.75);
    }
    if v > UI_SCALE_FLOOR {
        v
    } else {
        UI_SCALE_FLOOR
    }
}

impl UiScript {
    /// The host's `uiScale`, compared first like [`Self::set_screen_size`]: a new value applies at
    /// once, as the CVar's change callback does (`0x490770`), and `true` says it reached a
    /// `UIParent`. The same value again does nothing, so an addon's own `UIParent:SetScale` holds
    /// until the next change.
    pub fn set_ui_scale(&mut self, ui_scale: f32) -> bool {
        {
            let mut model = self.model_mut();
            if model.ui_scale_request == Some(ui_scale) {
                return false;
            }
            model.ui_scale_request = Some(ui_scale);
        }
        self.apply_ui_scale()
    }

    /// Apply the last [`Self::set_ui_scale`] to `UIParent` unconditionally, as the UI load does
    /// once its files and `VARIABLES_LOADED` are done (`0x49010e` → `0x492e90`) and a display
    /// change does (`0x492e90`, the window's aspect moving the cap). `false` with no value yet or
    /// no `UIParent`, where the reference's sink finds `[0xb4b44c]` null and returns.
    pub fn apply_ui_scale(&mut self) -> bool {
        let mut model = self.model_mut();
        let Some(ui_scale) = model.ui_scale_request else {
            return false;
        };
        let Some(ui_parent) = model.arena.lookup("UIParent") else {
            return false;
        };
        let aspect = model.screen.width() / model.screen.height();
        let v = effective_ui_scale(ui_scale, aspect);
        model.set_frame_scale(ui_parent, v);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_callback_ladder_caps_below_four_thirds_and_floors_at_the_sink() {
        // 4:3 and wider: the dial as set.
        assert_eq!(effective_ui_scale(0.9, 4.0 / 3.0), 0.9);
        assert_eq!(effective_ui_scale(0.9, 16.0 / 9.0), 0.9);
        assert_eq!(effective_ui_scale(1.2, 16.0 / 9.0), 1.2);
        // 5:4 caps a full dial at 0.9375 and leaves 0.9 alone.
        assert_eq!(effective_ui_scale(1.0, 1.25), 0.9375);
        assert_eq!(effective_ui_scale(0.9, 1.25), 0.9);
        // The floor, on both sides of it.
        assert_eq!(effective_ui_scale(0.5, 16.0 / 9.0), UI_SCALE_FLOOR);
        assert_eq!(effective_ui_scale(0.65, 16.0 / 9.0), 0.65);
        // A square window caps at 0.75, a narrow one bottoms out at the floor.
        assert_eq!(effective_ui_scale(0.9, 1.0), 0.75);
        assert_eq!(effective_ui_scale(0.9, 0.5), UI_SCALE_FLOOR);
    }

    #[test]
    fn the_scale_lands_on_uiparent_alone_and_only_on_a_change() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(1024.0, 768.0);
        assert!(!s.set_ui_scale(0.9), "no UIParent yet: nothing to scale");
        s.run(r#"CreateFrame("Frame", "UIParent") CreateFrame("Frame", "Loose")"#)
            .unwrap();
        assert!(!s.set_ui_scale(0.9), "the same value is no change");
        assert!(s.apply_ui_scale(), "the load edge applies what was set");
        assert_eq!(s.eval::<f32>("return UIParent:GetScale()").unwrap(), 0.9);
        assert_eq!(s.eval::<f32>("return Loose:GetScale()").unwrap(), 1.0);
        // An addon's own scale holds until the dial moves.
        s.run("UIParent:SetScale(0.7)").unwrap();
        assert!(!s.set_ui_scale(0.9));
        assert_eq!(s.eval::<f32>("return UIParent:GetScale()").unwrap(), 0.7);
        assert!(s.set_ui_scale(0.8));
        assert_eq!(s.eval::<f32>("return UIParent:GetScale()").unwrap(), 0.8);
    }
}
