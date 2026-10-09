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

/// The cap both `uiScale` legs apply before the sink (`0x490770`, and the display handler's ON leg
/// `0x492e90`): below a 4:3 aspect `a` the value is capped at `0.75·a` (`a·3 ≥ 4` skips the cap, so
/// 4:3 itself is never capped).
pub(crate) fn capped_ui_scale(ui_scale: f32, aspect: f32) -> f32 {
    if aspect * 3.0 < 4.0 {
        ui_scale.min(aspect * 0.75)
    } else {
        ui_scale
    }
}

/// `SetUiScale`'s floor (`0x494550`): above [`UI_SCALE_FLOOR`] the value stands, else (NaN
/// included) the floor.
pub(crate) fn floored_ui_scale(v: f32) -> f32 {
    if v > UI_SCALE_FLOOR {
        v
    } else {
        UI_SCALE_FLOOR
    }
}

/// What the CVar callback applies (`0x490770` → `0x494550`): capped, then floored.
pub(crate) fn effective_ui_scale(ui_scale: f32, aspect: f32) -> f32 {
    floored_ui_scale(capped_ui_scale(ui_scale, aspect))
}

/// The automatic scale `0x492f70` computes for a `w`×`h` display: 1.0, or `768/h` above 768 tall
/// (`0x492fbc cmp esi,0x300`), under the same narrow-aspect cap, then raised to 0.9 unless above it
/// (`0x493005`–`0x493012`).
pub(crate) fn auto_ui_scale(w: u32, h: u32) -> f32 {
    let mut v = if h > 768 { 768.0 / h as f32 } else { 1.0 };
    if h > 0 {
        v = capped_ui_scale(v, w as f32 / h as f32);
    }
    if v > 0.9 {
        v
    } else {
        0.9
    }
}

impl UiScript {
    /// The host's `uiScale`, compared first like [`Self::set_screen_size`]: a new value applies at
    /// once through the CVar's change callback (`0x490770`: capped, floored at 0.64), and `true`
    /// says it reached a `UIParent`. The same value again does nothing, so an addon's own
    /// `UIParent:SetScale` holds until the next change.
    pub fn set_ui_scale(&mut self, ui_scale: f32) -> bool {
        let applied = {
            let mut model = self.model_mut();
            if model.ui_scale_request == Some(ui_scale) {
                return false;
            }
            model.ui_scale_request = Some(ui_scale);
            let aspect = model.screen.width() / model.screen.height();
            model
                .arena
                .lookup("UIParent")
                .map(|h| (h, effective_ui_scale(ui_scale, aspect)))
        };
        let Some((ui_parent, v)) = applied else {
            return false;
        };
        self.model_mut().set_frame_scale(ui_parent, v);
        true
    }

    /// Record `ui_scale` without applying it, then apply it through the display handler, as the UI
    /// load does once its files and `VARIABLES_LOADED` are done (`0x49010e` → `0x492e90`): the
    /// CVar's own callback found no `UIParent` when it fired at registration.
    pub fn seat_ui_scale(
        &mut self,
        ui_scale: f32,
        display: (u32, u32),
        auto_cache: &mut Option<(u32, u32)>,
    ) -> bool {
        self.model_mut().ui_scale_request = Some(ui_scale);
        self.apply_ui_scale(display, auto_cache)
    }

    /// The display handler `0x492e90` on the last [`Self::set_ui_scale`], for the load and a
    /// display change. Its ON leg caps the value and hands it to the sink only at 0.64 or more
    /// (`0x492efd fcomp [0x804584]; test ah,1; jne 0x492f4a`); below, it falls to the automatic
    /// scale `0x492f70(w, h, force = 0)` ([`auto_ui_scale`]), which does nothing at all when the
    /// display is the one it last ran for (`auto_cache`, the reference's `[0xb4e304]`/`[0xb4e308]`,
    /// which outlive the UI). `display` is the window in logical px, the host's unit for the
    /// reference's device size. `true` when a scale reached `UIParent`.
    pub fn apply_ui_scale(
        &mut self,
        display: (u32, u32),
        auto_cache: &mut Option<(u32, u32)>,
    ) -> bool {
        let applied = {
            let model = self.model_ref();
            let Some(ui_scale) = model.ui_scale_request else {
                return false;
            };
            let aspect = model.screen.width() / model.screen.height();
            let v = capped_ui_scale(ui_scale, aspect);
            let v = if v >= UI_SCALE_FLOOR {
                v
            } else {
                let (w, h) = display;
                if w > 0 && h > 0 && *auto_cache == Some(display) {
                    return false;
                }
                *auto_cache = Some(display);
                floored_ui_scale(auto_ui_scale(w, h))
            };
            model.arena.lookup("UIParent").map(|h| (h, v))
        };
        let Some((ui_parent, v)) = applied else {
            return false;
        };
        self.model_mut().set_frame_scale(ui_parent, v);
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
        assert!(
            s.apply_ui_scale((1024, 768), &mut None),
            "the load edge applies what was set"
        );
        assert_eq!(s.eval::<f32>("return UIParent:GetScale()").unwrap(), 0.9);
        assert_eq!(s.eval::<f32>("return Loose:GetScale()").unwrap(), 1.0);
        // An addon's own scale holds until the dial moves.
        s.run("UIParent:SetScale(0.7)").unwrap();
        assert!(!s.set_ui_scale(0.9));
        assert_eq!(s.eval::<f32>("return UIParent:GetScale()").unwrap(), 0.7);
        assert!(s.set_ui_scale(0.8));
        assert_eq!(s.eval::<f32>("return UIParent:GetScale()").unwrap(), 0.8);
    }

    /// The automatic scale (`0x492f70`): 1.0 up to 768 tall, `768/h` above it, never under 0.9.
    #[test]
    fn the_auto_scale_follows_the_height_and_stops_at_nine_tenths() {
        assert_eq!(auto_ui_scale(1024, 768), 1.0);
        assert_eq!(auto_ui_scale(1280, 800), 0.96);
        assert_eq!(auto_ui_scale(1920, 1080), 0.9);
        // A tall narrow display: capped, then raised to 0.9.
        assert_eq!(auto_ui_scale(600, 700), 0.9);
    }

    /// The display handler (`0x492e90`) never floors: a dial capped below 0.64 falls to the
    /// automatic scale, which skips a display it already ran for; the CVar callback (`0x490770`)
    /// floors instead.
    #[test]
    fn below_the_floor_the_display_handler_falls_to_the_auto_scale() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(1024.0, 768.0);
        s.run(r#"CreateFrame("Frame", "UIParent")"#).unwrap();
        let scale = |s: &UiScript| s.eval::<f32>("return UIParent:GetScale()").unwrap();
        let mut cache = None;
        assert!(s.seat_ui_scale(0.5, (1280, 800), &mut cache));
        assert_eq!(scale(&s), 0.96, "the load took the auto scale for 1280×800");
        assert_eq!(cache, Some((1280, 800)));
        // The same display again: the auto path does nothing at all.
        s.run("UIParent:SetScale(0.7)").unwrap();
        assert!(!s.apply_ui_scale((1280, 800), &mut cache));
        assert_eq!(scale(&s), 0.7);
        // A new display runs it.
        assert!(s.apply_ui_scale((1920, 1080), &mut cache));
        assert_eq!(scale(&s), 0.9);
        // The CVar callback floors.
        assert!(s.set_ui_scale(0.55));
        assert_eq!(scale(&s), UI_SCALE_FLOOR);
        // At the floor or above, the display handler hands the value to the sink.
        assert!(s.seat_ui_scale(0.8, (1920, 1080), &mut cache));
        assert_eq!(scale(&s), 0.8);
    }
}
